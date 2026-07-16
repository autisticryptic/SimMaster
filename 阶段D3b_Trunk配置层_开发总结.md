# 阶段 D3b 开发工作总结：每线路 SIP Trunk 配置层与 API

> **后续进展（2026-07-16）**：D3b-runtime 与 D3b-UI 已由 `2b78d5f` 完成；D4 SIP UDP endpoint、Digest REGISTER、刷新/退避、静态 Peer 监听已由 `47b57d4` 完成，并在 `6d10cae` 补齐稳定本地 Contact 与关闭时注销。详细记录见 `阶段D4_SIP端点与Asterisk注册_开发总结.md`。

> **文档性质**：单阶段开发工作记录（工作成果 + 剩余待办）
> **日期**：2026-07-16
> **对应规划**：`SimAdmin_扩展开发文档_多路径语音短信Trunk_进度更新版_v2_视频切换与Trunk对接.md` 第二十章 + 阶段 D（每线路 SIP Trunk 网关）
> **本轮范围**：阶段 D 的第一刀 —— **D3b 配置模型 + API**（纯离线、默认关闭、无真机依赖）。SIP endpoint / RTP bridge / 语音 INVITE 真机接线均**不在本轮**。

---

## 一、背景与决策依据

本轮开发落地的是 2026-07-16 敲定的三项架构决策中、可在 Windows 上纯离线完成的第一部分：

- **线路 B + 画法一**：SimAdmin 作 SIP Trunk 对接远程 Asterisk，Web 电话挂在 Asterisk 后面，转码（AMR↔Opus）与 WebRTC 终结全交 Asterisk，SimAdmin 保持纯 RTP relay。
- **Trunk 注册双模可配**：`static_peer`（IP 直连、被动应答）与 `outbound_register`（主动 REGISTER + 定时刷新）都实现，用户按环境在 `TrunkProfile.registration_mode` 里选。
- **来电策略归属 Asterisk**：运营商来电进入 SimAdmin 后不做号码/内容筛选，按 `line_id` 直接桥接到 Asterisk；黑白名单、验证码识别、营销拦截、振铃组和语音信箱后续统一由 Asterisk 侧处理。
- **Web 软电话最终 Todo**：只连接 Asterisk WSS，与 SimAdmin 后端零耦合；当前阶段不开发、不阻塞 Trunk 主线。

**为什么先做配置层**：阶段 D 后续的 `trunk/sip_endpoint.rs`、`bridge.rs`、语音 INVITE 真机接线都依赖 aarch64 Linux 设备才能验证，Windows 上只能编译不能真跑。按文档 §14 工程规范（每阶段独立可编译 + 单测、新功能默认关闭、小步提交），正确的第一刀是纯离线、零风险、且是后续所有 trunk 代码地基的**配置模型 + API**。

**关键设计判断**：现有 `LineProfileConfig` 的注释本身写着 *"Trunk settings will extend this same profile later"*，因此 trunk 配置作为字段挂进 `LineProfileConfig`（顺作者预留意图），而非另起结构。

---

## 二、已完成的工作

### 2.1 配置层 —— `backend/src/infra/config.rs`

**新增类型：**

- `TrunkRegistrationMode`（enum，`#[serde(rename_all = "snake_case")]`）
  - `StaticPeer`（默认）：两端互钉 IP:port，不 REGISTER，靠 `match_host` 认对端；SIP 请求仍为双向，SimAdmin 可在运营商来电时主动向 Asterisk 发 INVITE。
  - `OutboundRegister`：主动注册为 Asterisk endpoint，每 `register_expiry_secs` 刷新，NAT 友好。
- `TrunkProfileConfig`（struct，全字段 `#[serde(default)]`，默认惰性/禁用）
  - 通用字段：`enabled` / `registration_mode` / `asterisk_host` / `asterisk_port`(默认 5060) / `username` / `secret` / `context` / `extension` / `codec_allow` / `register_expiry_secs`(默认 3600) / `match_host`。
  - `redacted()`：返回 secret 置空的副本，用于跨 API 边界。
  - `secret_set()`：是否已存 secret（供 UI 提示而不泄漏值）。
- `LineProfileConfig` 扩展：新增 `trunk: TrunkProfileConfig` 字段；`for_line()` 同步初始化；新增 `redacted()`（连带 trunk 脱敏）。

**新增 ConfigManager 方法：**

- `set_line_trunk_profile(line_id, TrunkProfileConfig)`：替换整条线路的 trunk 设置。门禁校验仿 `set_line_volte_connection_enabled` —— 启用要求线路已启用 + `asterisk_host` 非空；`OutboundRegister` 模式额外要求 `username` 非空。**空 secret = 保留已存 secret**（支持前端脱敏回环，不会误清凭据）。`line_id` 走 `valid_line_id` 校验，写后 `save()` 持久化。
- `set_line_trunk_enabled(line_id, bool)`：仅切换开关，不重交完整设置；启用时重校验已存 profile，避免半配置的 trunk 被打开。

**新增单测（7 个）：**

| 测试 | 验证点 |
|------|--------|
| `trunk_defaults_are_inert_and_off` | 默认禁用、端口 5060、expiry 3600、模式 StaticPeer |
| `trunk_enable_requires_asterisk_host` | 启用缺 host → `trunk_asterisk_host_required` |
| `trunk_outbound_register_requires_username` | outbound 缺 username → `trunk_username_required` |
| `trunk_invalid_line_id_rejected` | 非法 line_id → `invalid_line_id` |
| `trunk_static_peer_persists_and_redacts_secret` | 落盘保留 secret；`redacted()` 不含 secret |
| `trunk_empty_secret_keeps_stored_secret` | 空 secret 提交保留旧 secret，其它字段更新 |
| `trunk_toggle_revalidates_stored_profile` | toggle 启用重校验；配置齐后可开可关 |

### 2.2 API 层 —— `backend/src/api/handlers.rs`

- 新增 `TrunkProfileResponse`：`line_id` + 脱敏 `trunk` + `secret_set` 提示位。
- 新增 3 个 handler（仿 vilte handler 风格）：
  - `get_line_trunk_handler`（GET）：读一条线路 trunk 设置，未配置线路返回惰性默认，保证 UI 有 shape。
  - `set_line_trunk_handler`（POST）：替换 trunk 设置，config 层错误串原样透传供 UI 映射。
  - `set_line_trunk_enabled_handler`（POST）：开关切换。
- **安全修复（顺带）**：原 `build_volte_line_response` 与 `set_volte_line_connection_handler` 直接内嵌 `LineProfileConfig` 序列化；trunk 现带 secret，已在这两处补 `.redacted()`，堵住 `/api/volte/lines*` 的潜在泄漏。

### 2.3 路由 —— `backend/src/main.rs`

在 auth 中间件保护的 `protected_routes` 内注册：

- `GET/POST /api/trunk/lines/{line_id}`
- `POST /api/trunk/lines/{line_id}/enabled`

（handler 经 `use api::handlers::*` 引入，无需改 import。）

### 2.4 安全与兼容性

- **默认关闭**：`TrunkProfileConfig::default()` 全惰性，旧配置文件反序列化后 trunk 保持禁用。
- **凭据不外泄**：secret 落盘但所有 API 响应经 `redacted()`；`secret_set` 仅暴露"是否已配"。
- **向后兼容**：全字段 `#[serde(default)]`，且经核查后端源码无其它 `LineProfileConfig` 结构字面量，新增字段不会破坏既有构造。

---

## 三、验证状态

> **2026-07-16 验证完成**：修复了测试插入时误删 `default_ddns_provider()` 函数头、`TrunkProfileResponse` 缺少 `Default` 以及派生 `Default` 导致端口/注册周期为 0 三个问题。当前配置/API 层已经通过编译和完整回归。

**待执行的验证（命令通道恢复后立即补做）：**

- [x] `cargo test trunk` —— 7 个配置单测及 1 个现有 Trunk seam 单测全绿（项目无 library target，原 `--lib` 命令不适用）。
- [x] `cargo test` —— 492 项全量测试通过。
- [x] `cargo clippy --all-targets -- -D warnings` 零告警。
- [x] `cargo fmt --all --check` 通过。
- [x] **git commit**：`b675d32 feat(trunk): add per-line trunk profile API`。

**已知需真机验证的边界**：本轮不涉及任何真机 IO，无 aarch64 依赖，Windows CI 单测即可全量覆盖。

### 3.1 高通 410 隔离验证（2026-07-16）

- 使用 `cargo zigbuild --release --target aarch64-unknown-linux-musl` 完成交叉编译；候选提交 `b675d32`，SHA-256 为 `294D5E95C0AF43A9E290AA9B09DDC9AB688CB050C96A8B6B39B6B4BC50B66B0F`。
- 候选部署到 `/opt/simadmin/releases/b675d32-d3b/simadmin`，仅监听设备本机 `127.0.0.1:3101`；正式 `simadmin.service` 全程保持 inactive。
- 真机正确识别一条稳定线路；默认 Trunk 为关闭、`static_peer`、端口 5060、注册周期 3600 秒。
- 验证 outbound register 缺用户名会返回 `trunk_username_required`；禁用状态可保存完整配置。
- 测试凭据确实写入隔离配置，但 GET/POST/toggle 以及 `/api/volte/lines` 响应中的 `secret` 始终为空，`secret_set=true`；重启候选后配置与提示位保持。
- D3b 只有配置层，因此测试开关不会创建 SIP socket、不会解析 Asterisk 地址，也不会发送 REGISTER，符合当前能力边界。
- 测试结束后临时服务停止、端口 3101 关闭、测试配置和数据库删除；`wwan0` 保持 DOWN、XFRM state/policy 为 0/0、ModemManager active。设备仅保留已验证候选二进制。

---

## 四、剩余未完成的工作

### 4.1 阶段 D 内剩余（trunk 主体）

| 子项 | 内容 | 依赖 | 说明 |
|------|------|------|------|
| **D3b-UI** | 前端 trunk 配置页（每线路：模式选择、Asterisk 地址/凭据、context/extension、codec allow-list、开关） | 本轮 API | 仿 `ModemLinesPanel.tsx` 的数据流；secret 输入框配合 `secret_set` 显示"已配置"态 |
| **D3b-runtime** | `LineRuntime` 新增 `trunk` 运行时字段（仿 `volte`/`volte_live`/`volte_connect_lock` 三件套挂载） | 本轮配置 | 目前只有配置，尚无运行时持有者 |
| **D4** | `trunk/sip_endpoint.rs`：对内 SIP UAS/UAC/B2BUA + REGISTER 客户端（双模）；每线路独立鉴权与 Asterisk 路由 | D3b-runtime | ⚠️ 真机验证；复用 `access/volte/sip.rs` 报文构造器（`build_invite:317` 等，传输无关） |
| **D5** | `trunk/rtp_relay.rs`：RTP 双向转发 | D4 | 复用 `access/volte/rtp_relay.rs::RtpRelayCore`（媒体无关）+ `io::run_relay`；音频/视频各一实例 |
| **D6** | `trunk/bridge.rs`：内外腿桥接，实现 `TrunkVideoSeam`（`vilte.rs:354`）；双向 re-INVITE 桥接 | D4/D5 | 把内部 UA 视频端点喂给 `VideoRelay` |
| **D7** | 强制鉴权 + 内网默认绑定 + 与本地 Asterisk/Linphone 联调（先语音，再 ViLTE） | D4-D6 | ⚠️ 安全项；⚠️ 真机验证、抓包比对 |

### 4.2 语音链路缺口（阶段 E 残留，trunk 联调的最大新工作量）

- **语音 INVITE 真机接线**：`access/volte/live.rs` 目前只有注册 + 短信的 socket 装配，**没有把语音 INVITE 真正发到运营商 IMS 的 live 接线**（与文档 §十一 阶段 E3「未做」一致）。离线层（`VolteVoiceCall`、SDP、re-INVITE）已就绪，缺"把算好的字节经真实 socket 收发 + 驱动状态机"的真机层。这是 trunk 能真正拨通电话前的必经工作。

### 4.3 Web 软电话（最终 Todo，当前不开发）

- 独立静态页面，倾向 Cloudflare Pages/Workers 托管，SIP.js/JsSIP 连 Asterisk WSS，与 SimAdmin 后端零耦合。
- WSS / ICE / DTLS-SRTP / TURN 全部由 Asterisk 承担，页面侧只做 UI + 信令客户端。
- 承载方式首选 Cloudflare Pages 独立部署；本地 `www/webphone/` 子目录（现有 `spa_fallback` 可直接服务）作为离线兜底。

### 4.4 贯穿性

- [ ] 每阶段更新开发/API 文档（本文档 + v2 规划文档已更新；`bruno-api` REST 映射表待补 trunk 三个端点）。
- [ ] trunk 相关对外端点鉴权审查（D7 前必做）。
- [ ] 分阶段 git 提交检查点（本轮提交待命令通道恢复后补）。

---

## 五、下一步建议顺序

1. **Git 检查点 + ARM64 D3b 真机 API 验证**：验证配置持久化、凭据脱敏、旧配置迁移和多线路隔离，不会尝试 REGISTER。
2. **D3b-runtime + D3b-UI**：把 trunk 运行时挂进 `LineRuntime`，做前端配置页。
3. **D4/D5/D6（trunk 主体）**：实现双模 SIP endpoint、RTP relay 和桥接；运营商来电不经过 SimAdmin 筛选，直接转给 Asterisk。
4. **语音 INVITE 真机接线（§4.2）**：与 D4 并行或前置，是拨通电话的前提。
5. **Web 软电话**：仅作为所有 Trunk/语音/视频工作之后的最终 Todo。

---

## 六、涉及文件清单

| 文件 | 改动 |
|------|------|
| `backend/src/infra/config.rs` | 新增 `TrunkRegistrationMode` / `TrunkProfileConfig`；扩展 `LineProfileConfig`（+`trunk` 字段、`redacted()`）；新增 `set_line_trunk_profile` / `set_line_trunk_enabled`；新增 7 个单测 |
| `backend/src/api/handlers.rs` | 新增 `TrunkProfileResponse` + 3 个 trunk handler；`LineProfileConfig` 导入补 `TrunkProfileConfig`；`build_volte_line_response` 与连接 handler 补 `.redacted()` 脱敏 |
| `backend/src/main.rs` | 注册 `/api/trunk/lines/{line_id}` 与 `/api/trunk/lines/{line_id}/enabled` 路由 |
