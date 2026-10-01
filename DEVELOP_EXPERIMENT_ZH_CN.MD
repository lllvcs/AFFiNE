# AFFiNE 自托管分支 — 开发记录

> **这份文件是什么。** 记录本分支相对上游改了什么、以及改动过程中真实发生过什么：每次故障、实际是怎么定位的、哪些验证过、哪些没有。写给**准备改这份代码、需要历史而不是规则**的开发者。
>
> 从这些记录里提炼出的**规则**在 [`DEVELOP_SKILL.MD`](./DEVELOP_SKILL.MD)（英文，面向 agent）。配置与部署**用法**见 [`README.zh-CN.md`](./README.zh-CN.md)。

- 基线：上游 AFFiNE 源码，版本 `0.27.5`，分支 `canary`
- 镜像：`ghcr.io/lllvcs/affine`（构建产物）→ `lvcs/affine`（发布产物）
- 本机没有 docker、也没有旧客户端：协议层与端到端行为**只能在部署实例上验证**

---

## 1. 改动清单

| 主题 | 提交 | 改了什么 |
| --- | --- | --- |
| 自托管配额放开 | `c5844b1` | blob 10 GiB、存储配额 2^53−1、坐席上限 `i32::MAX`、历史 100 年 |
| BYOK + Copilot 诊断 | `c5844b1` `a16e5f3` `4d1f8b8` | 可列出 BYOK 提供方的模型；路由失败带上槽位、所需能力、部署类型与 BYOK 状态；界面明示"对话需文本输出**且**支持 tool calling" |
| OIDC 兼容不返回 `email_verified` | `42bbe52` | `trustUnverifiedEmail`（显式 `false` 仍拒绝）、错误原因细化、3 个 Rust 单测 |
| OIDC 回调地址 | `c9d5013` | 协议跟随请求：externalUrl 主机 → 请求协议（仅白名单主机）→ `server.https` |
| 登录方式开关 | `28ac1be` `8bc95d7` `b57a6b9` `4b5a1c1` `7100627` `ed18a8c` | `auth.signInMethods.{password,magicLink,oauth}`，`config.json` 与环境变量均可读，服务端强制 |
| 实时同步兼容已发布客户端 | `9b9f22e` `6912abd` | 重新提供旧 `space:join` 与 `sync-025`/`sync-026` 房间；join 被拒时打印客户端版本 |
| 自托管文档 | `9bb4520` `f422eeb` `3fb9695` | `server.hosts` 语义、两个配置读取方、两份名单规则 |
| 镜像流水线 | `b4781c0` … `fdf8f1b` | 构建与发布分离、仓对仓复制、Pro 公钥守门 |

---

## 2. 故障记录

每条格式：现象 → 实际是什么 → 什么解决了它。

### 2.1 所有客户端完全无法同步（AI 面板同时报 `no compatible target`）

**现象：** 客户端连上就断；新建工作区在另一端看不到；工作区自己的根文档报 `DOC_NOT_FOUND`。

**实际原因：** 0.27.5 用 `space:join-batch` 取代了基于房间的同步协议，并**删除了**旧握手。已发布客户端（0.27.4 桌面端、0.27.1 手机端）因此 join 被拒——而且这个拒绝**没有任何日志**。

**解决：** 加法式回移旧协议（`9b9f22e`）+ 打印被拒版本的日志（`6912abd`）。根文档要等客户端能推上去才存在，所以那个 404 是症状而不是独立故障。

### 2.2 登录报 `INVALID_OAUTH_RESPONSE`

**原因：** native 侧强制要求 `email_verified` claim；群晖 SSO 声明的是 `aud, email, exp, groups, iat, iss, sub, username`，从不返回它。

**解决：** `oauth.providers.oidc.trustUnverifiedEmail`（默认失败即关闭，显式 `false` 仍拒绝）+ 错误原因细化，避免下次再从一句笼统的话开始查。

### 2.3 TLS 反代后 `redirect_uri` 变成 `http://`

**原因：** origin helper 跟随了请求**主机**，协议却写死取 `server.https`——那描述的是"服务器怎么监听"，不是"客户端怎么访问"。

**解决：** 协议来源排序（`c9d5013`）。注意**没有**做的事：打开 `trust proxy` 会让框架信任客户端自报地址，并静默削弱 IP 限流。

### 2.4 登录开关：六个提交、四次走错方向

这是本分支里最有价值的一段，完整记录。

| 轮次 | 当时的判断 | 实际情况 |
| --- | --- | --- |
| 1 | 镜像还没包含改动 | TS 代码确实在镜像里（对 bundle `grep -c`）、环境变量也确实注入——但两者都**不能证明值被读取** |
| 2 | 关键在"值写在哪里" | 环境变量被跳过，因为这个键登记成 native 所有、而强制逻辑在 TS |
| 3 | 修好所有权就行 | 环境变量通了、**文件反而失效**：Node 层根本不读 `config.json` |
| 4 | 混合方案（native 所有 + env 兜底）两边都能用 | 代码层面两边都通了，值**依然没到** |

**真正的原因：** native 运行时解析 `config.json` 并持有该值，但只有 `node_owned()` 那份**投影**会跨到 Node 层——而 `auth.signInMethods.*` 不在投影名单里。值被解析、被保存、从不交付。

**结束循环的东西：** 一行打印解析值的日志（`7100627`）——文件里写的是 `false/false`，日志显示 `password=true magicLink=true oauth=true`。就凭这一行，读 `file.rs` 就找到了缺口。

**教训**（已成规则 R1、R2、R3、R5）：所有权决定**来源**，投影决定**交付**；当解析值在线缆上不可观测时，加日志而不是再加一个猜想。

### 2.5 服务端因 `crypto.privateKey` 崩溃重启

**原因：** 开启持久化 BYOK 会激活启动期校验（要求稳定密钥）；第一次交出去的是随机字符串——native 接受，Node 层不接受（`1E08010C:DECODER routines::unsupported`）。

**解决：** 用镜像自带的 Node 生成真正的 EC P-256 PEM；数据库 `app_configs` 里已有旧值时优先复用（换新 key 会让已存凭据失效）；并文档化"必须是单行 JSON、`\n` 转义"。

### 2.6 所有入口的 CORS/WebSocket 被拒

**原因：** `server.hosts` 没有写实际使用的入口，而这份列表就是允许来源集合。写成 URL 形状则永远匹配不上（会被拼成 `http://http://host`）。

**解决：** 纯配置——裸主机名、不带协议，并以启动日志 `Telemetry allowed origins updated: …` 校验。

---

## 3. 架构要点

- **配置所有权。** Node 层先解析环境变量与默认值（`getDefaultConfig`），随后叠加 native 运行时的配置，最后叠加 Node 侧覆盖。native 所有的键在第一步被**跳过**——除声明了 env 名的键（`4b5a1c1` 之后），因此环境变量可作兜底、文件仍然优先。
- **native 两份名单。** `NATIVE_APP_CONFIG_KEYS`（存储/校验）与 `node_owned()`（投影给 Node）。两侧都消费的键必须同时登记。
- **两条配置路径。** Node 优先 `/app/config.json`，否则 `$HOME/.affine/config/config.json`；native 认 `AFFINE_BACKEND_RUNTIME_CONFIG_PATH`。要让两者指向同一份文件。
- **同步房间。** `sync-025`（0.25，逐条）、`sync-026`（0.26/0.27，批量）、`sync-027`（0.27.5，内存态订阅 + invalidation）。旧客户端靠房间成员识别，并按**每请求**鉴权。
- **版本。** 客户端版本是构建标签；同步网关对批量协议要求 `>=0.27.5`，而由服务端托管的前端上报同一标签——所以改标签从来不是"降级"。

---

## 4. 验证手册

```sh
# Rust（native 层）——cargo 可能不在 PATH
CARGO_HOME=E:/AFFiNE/.cargo RUSTUP_HOME=$HOME/.rustup \
  CARGO_TARGET_DIR=E:/AFFiNE/.scratch/target \
  $HOME/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/cargo.exe \
  check -p affine_server_native --lib

# TypeScript（服务端）
node_modules/.bin/tsc -p packages/backend/server/tsconfig.json --noEmit
node node_modules/oxlint/bin/oxlint <改动文件>
```

这里产生过错误结论的陷阱：

1. 复合工程 + 陈旧 `dist/tsconfig.tsbuildinfo` 会给出**上一版内容**的结论——结果可疑时删掉重跑。
2. 用 `tsc … --listFiles | grep <文件>` 才能证明某文件真的被编译（命中 1 次即在内）。
3. 对 minified bundle 用 `grep -c` 数的是**行数**，不是次数。
4. Windows 文件系统大小写不敏感：`rm NAME.MD` 会把 `NAME.md` 一起删掉。
5. 没有 docker 就没有本地端到端——这一点必须写清楚。

---

## 5. 与用户协作的经验

1. 一次只推进一个问题；顺序由用户指定，其余用一行"仍待处理"挂起。
2. 给**能判读结论**的命令，并附"结果 → 结论"对照表。让用户粘贴一条 `docker compose exec … psql …` 远好于让他读设置页。
3. **先读代码核实契约，再让用户改生产配置**——本次多数返工都来自"想当然"。
4. 提交信息写清：现象、影响面，以及 `Verified:` / `Not verified:` 分列。

---

## 6. 未完成项

- 登录开关**没有自动化测试**（auth 的 spec 需要活数据库与 native 模块）。
- `db.datasourceUrl` 归 native 所有但**未投影**；服务端改读 `DATABASE_URL`。
- 旧同步协议兼容层**没有**对真实 0.27.4 桌面端 / 0.27.1 手机端的端到端验证。
- 写下这份记录时，登录开关在实例上的行为仍在确认中。
