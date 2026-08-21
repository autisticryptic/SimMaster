# 多 UE 隔离架构迁移文档（Option B：per-UE worker + setns）

> 状态：**阶段一/二（worker 底座 + VoWiFi 数据面）已完成代码实现，待 410 单 UE 实机验证**。
> 本文档是 `multi_ue_ims_volte_vowifi_architecture.md` 的落地实现记录，记录了已完成的
> 实机验证、当前代码状态、控制协议，以及 VoWiFi → VoLTE → 数据代理/Trunk → 5G
> 的逐步迁移计划与验收标准。

---

## 1. 目标与已确认的路线

### 1.1 问题

多 SIM / 多基带插在同一台宿主上时，多个 UE 可能拿到完全相同的：

- 运营商分配的 IP / 网关
- P-CSCF 地址
- IMS 注册状态、SIP dialog、IKE/IPsec/XFRM 状态、RTP 会话

旧实现把 IP 地址当作区分的依据，并在宿主 netns 里用策略路由/绑定网卡来勉强分流。
一旦两个 UE 的 IP、网关、P-CSCF 完全相同，Linux 路由、ARP、XFRM 状态就会互相干扰。

### 1.2 已确认的方案（Option B）

用户已确认采用 Option B：

> 每个 UE = 一个 `UeContext` + 一个独立 Linux `Network Namespace`，
> 由主进程拉起一个 `simadmin ue-worker` 子进程，子进程通过 `setns(CLONE_NEWNET)`
> **在进入 UE netns 之后才创建任何 socket**。

迁移顺序（用户确认）：

1. **VoWiFi**：TUN / IKE / XFRM / RTP 已全部用户态实现，在 worker netns 内最自洽，
   先完成单 UE 验证。
2. **VoLTE**：`wwanX` 由基带创建在宿主 netns，需要 veth 桥接，或把
   `VolteSipChannel`/register 迁进 worker（核心 IMS 重构，逐步实机回归）。
3. **数据代理与 Trunk 映射**：per-UE proxy 在 UE netns 内监听，出口与绑定一一对应。
4. **5G IMS**：直接挂在同一套 worker 模型上。

核心原则：**UE 是第一等公民，IP 不是 UE 的唯一身份；每个 UE 独立维护
Network / IMS / IKE / RTP 状态。**

---

## 2. 总体架构

```text
                    simadmin 主进程（宿主 netns）
   ┌─────────────────────────┬──────────────────────────────┐
   │  ModemManager / QMI      │  API / DB / 事件总线          │
   │  bearer 生命周期          │  线路注册、Trunk、自动化        │
   └───────────┬─────────────┴──────────────────────────────┘
               │ spawn + setns(CLONE_NEWNET)
   ┌───────────▼────────────────────────────┐
   │  simadmin ue-worker  (UE netns)        │
   │  ┌──────────────────────────────────┐  │
   │  │ 每个 UE 自己的网络栈：             │  │
   │  │  - veth UE 侧 (save<hex>)        │  │
   │  │  - TUN 内网地址 + P-CSCF 路由     │  │
   │  │  - SIP / RTP / IKE socket        │  │
   │  │  - (后续) wwanX、per-UE proxy    │  │
   │  └──────────────────────────────────┘  │
   └───────────┬────────────────────────────┘
               │ JSON-lines Unix socket（控制通道，SCM_RIGHTS 传 fd）
   ┌───────────▼───────────────┐
   │ 宿主侧 veth 对 (savh<hex>) │  → NAT → WiFi / 默认出口
   └───────────────────────────┘
```

### 2.1 命名与拓扑（确定性，重启不变）

| 对象 | 规则 | 示例 |
|---|---|---|
| netns | `sa-ue` + line_id 的 md5 前 12 hex | `sa-ue3f9a2b1c7d4e` |
| 宿主侧 veth | `savh` + 后 8 hex | `savh2b1c7d4e` |
| UE 侧 veth | `save` + 后 8 hex | `save2b1c7d4e` |
| veth 地址 | `10.200.<a>.<b&0xFC>/30`（host），`+1`（UE） | `10.200.15.4` / `10.200.15.5` |
| TUN | 沿用 `tun_name_for_line()` | `sa_vwf<hex>` |

命名稳定 ⇒ 重启后可以回收/重建同一个 netns、veth 和 TUN，不会越线。

### 2.2 worker 进程边界

- **主进程负责**：硬件访问（ModemManager/QMI）、bearer/PDP 生命周期、配置、API、
  DB、事件总线、Trunk/Asterisk、用户态 ESP/TUN 转发器（fd 跨 netns 引用同一设备）。
- **worker 负责**：UE netns 内的所有 socket（IKE、SIP、RTP 等）与网络配置执行。
- **控制通道**：Unix socket，长度前缀 JSON 帧；fd 通过 `SCM_RIGHTS` 传递。

为什么不是把整个 IMS 状态机搬进 worker：VoLTE 的 bearer/QMI 仍在主进程，
把状态机拆成两半会放大重构风险。先只迁移"必须活在 UE netns 里的 socket 与网络配置"，
IMS 状态机保持在主进程，通过 fd 引用 UE netns 内创建的 socket。

---

## 3. 410 实机验证记录（2026-08，已完成）

设备：`192.168.100.13`（有时临时切换为 `192.168.68.1`），密码 SSH。

### 3.1 系统与 Modem 状态

- 内核 `6.17.0-rc6-lkiuyu-compile+`，aarch64
- `mmcli 1.24.0`，Modem `/Modem/3`，已连接，运营商 `50212`（MY MAXIS）
- `wwan0` 活跃，`10.210.45.180/29`，IPv6 已配置
- `wwan1..wwan7` 为 raw-IP `bam-dmux` 接口（空闲，可被占用）

### 3.2 netns 操作验证

| 操作 | 结果 |
|---|---|
| 把活跃 `wwan0` 移入 netns | 成功；ModemManager 保持 bearer 连接 |
| 把 `wwan0` 移回宿主 netns | ModemManager 重建 Modem（`Modem/0 → … → Modem/3`）并自动重连；SimAdmin 必须处理重新探测 |
| 把空闲 `wwan1` 移入 netns | 不干扰 ModemManager ⇒ **VoLTE 阶段优先占用空闲 `wwanN`，保留 `wwan0` 给默认数据** |
| netns 内手工配地址+默认路由 | 成功；ping `58.71.136.20` 约 14–125ms |

结论：

- `setns`/`ip netns` 在 410 上完全可用；
- VoLTE 阶段**不要动 ModemManager 正在使用的 `wwan0`**，用空闲 `wwan1..7`；
- ModemManager 对 `wwan0` 移出行为的重探测周期需要被 SimAdmin 容忍（重新 probe 期间
  线路暂时离线是预期的）。

---

## 4. 当前代码状态（本阶段已完成）

### 4.1 已落地模块

| 文件 | 内容 |
|---|---|
| `platform/netns.rs` | `NetnsName` 稳定命名、`ensure`/`remove`、`setns_pre_exec`、veth 对创建/拆除、单调命名检查 |
| `services/ue_context.rs` | UE 身份模型：`ue_id`、`kind`（Modem / PCSC / 传统读卡器）、`uim_slot`、namespace、隔离开关状态 |
| `services/ue_worker.rs` | worker 进程管理、Hello 握手、`NetStatus`、`NetConfigRequest/Result` 关联批处理、`Ping/Pong`、优雅退出；**socket 工厂已实现（见 §5）** |
| `services/ue_netcfg.rs` | 纯函数规划器：veth 地址/名称、UE 侧 ops、TUN ops、wwan ops（可单元测试） |
| `connectivity/.../vowifi/live.rs` | 每线路 UE namespace 注册表 + worker 注册表 + **socket context 注册表**；IKE/SIP socket 按 context 选择 worker 创建或宿主路径 |
| `connectivity/.../vowifi/operator.rs` + `connectivity/core/media.rs` | **RTP/RTCP operator 侧 socket 通过 `OperatorSocketCreator` 走 worker**；Asterisk 内部 leg 仍留在宿主 |
| `connectivity/.../vowifi/tun_gateway.rs` | TUN 创建后 `ip link set ... netns <ns>`，netns 内配地址/路由；`None` 时代码保持旧宿主路径 |
| `services/line_registry.rs` | 线路刷新时 `reconcile_ue_context()`：ensure netns → spawn worker → veth → worker 应用 UE 侧配置 → 注册命名空间/socket context；关闭隔离时同时清理两个注册表 |
| `platform/netns.rs` | `ensure_host_veth_nat()`：宿主侧 MASQUERADE（幂等检查后追加） |
| `platform/config.rs` | `ue_isolation` 配置块（见 §6） |

### 4.2 worker 控制协议（当前）

消息（JSON-lines，`type` 区分）：

- `hello`：worker → 主进程（line_id / netns / pid）
- `net_status_request` / `net_status`：netns 内接口/地址/默认路由快照
- `net_config_request{request_id, ops}` / `net_config_result{outcome}`
- `socket_create_request{request_id, spec}` / `socket_create_result`（**fd 通过 SCM_RIGHTS 随帧传递**）
- `ping` / `pong`、`shutdown{reason}`

`NetConfigOp`（在 worker 自身 netns 内执行，有序、失败即中止并回报）：

- `link_set_up / link_set_down`
- `addr_replace / addr_del`（幂等）
- `route_replace / route_del`、`default_route_replace`、`flush_routes`

附加设计：`addr_del`、`route_del`、`flush_routes` 的“不存在”类错误视为良性，保证重入安全。

### 4.3 帧格式与 fd 传递（Unix）

```text
帧 = [u32 LE payload_len][payload(JSON)]
sendmsg 一次发送整帧；SocketCreateResult 把 fd 放在同一帧的 cmsg SCM_RIGHTS 中。
接收侧先 MSG_PEEK 等够整帧，再 recvmsg 精确消费一帧并收取 cmsg。
```

要点：

- 每帧一次 `sendmsg`，接收端每次恰好读一帧，不会把 fd 粘到别的消息上；
- worker 内创建的 socket 属于 UE netns（socket 的 netns 在创建时固定），
  fd 传给主进程后仍属于该 netns —— 这是"主进程持有 IMS 状态机、socket 却在 UE 栈里"
  的实现基础；
- 非 Linux 平台 `create_socket` 直接返回 `Unsupported`，宿主路径完全不变。

### 4.4 宿主侧 egress 与 NAT（已实现）

veth 对配置地址并 up 后，**宿主要把 UE 子网流量转发到 WiFi/默认出口必须加 SNAT**，
否则 ePDG 回包的目标 IP（`10.200.x.y`）在运营商侧不可路由。`platform/netns.rs` 新增
`ensure_host_veth_nat()`，在 veth 配置成功后幂等检查并追加规则；失败只告警，回退宿主路径：

```bash
sysctl -w net.ipv4.ip_forward=1
iptables -t nat -C POSTROUTING -s 10.200.a.b/30 -j MASQUERADE 2>/dev/null \
  || iptables -t nat -A POSTROUTING -s 10.200.a.b/30 -j MASQUERADE
# teardown:
iptables -t nat -D POSTROUTING -s 10.200.a.b/30 -j MASQUERADE 2>/dev/null || true
```

（nftables 环境使用 `nft add/delete rule ip nat postrouting ... masquerade`，见 §7 后续步骤。
410 的 iptables/nft 可用性需要在实机清单里确认。）

---

## 5. 阶段二 b：VoWiFi 数据面迁入 worker netns（代码已完成）

### 5.1 为什么 TUN 移进 netns 还不够

上一轮已实现：TUN 设备在创建后被 `ip link set ... netns <ns>` 移入 UE netns，
打开着的 fd 留在主进程，用户态 ESP 转发器继续工作。**但 SIP/RTP/IKE socket 仍在主进程
创建，并依赖 `SO_BINDTODEVICE(TUN)`——而 TUN 已经不在宿主 netns 里了，socket 绑不上，
注册必然失败。** 这就是 `vowifi_tun_in_namespace` 默认关闭的原因。

### 5.2 解决方案：worker socket 工厂

让 worker 在 UE netns 内创建并初始化 socket，再把 fd 传回主进程：

```text
主进程                        worker（UE netns）
  │ socket_create_request ─────► 创建 socket2
  │                              bind / bindsock(SO_BINDTODEVICE)
  │                              connect（TCP 带超时）
  │ ◄── socket_create_result ─── sendmsg(SCM_RIGHTS fd)
  │ 包装成 tokio socket 使用
```

`UeSocketSpec` 字段：

- `kind`: `udp | tcp`
- `family`: `ipv4 | ipv6`
- `bind`: 本地地址（可为 `0.0.0.0:port`）
- `connect`: 对端地址（UDP 等价 connect；TCP 阻塞 connect 带超时）
- `bind_to_device`: UE netns 内接口名（TUN 或 veth UE 侧）
- `reuse_address`
- `connect_timeout_secs`

实现现状：worker 侧用 `socket2` 创建 socket，按 `reuse_address` →
`SO_BINDTODEVICE` → `bind` → UDP `connect` / TCP `connect_timeout` 的顺序初始化，
`SocketCreateResult` 通过同一帧的 `SCM_RIGHTS` 把 fd 传回主进程；主进程侧阻塞线程用
`recvmsg(MSG_PEEK)` 保证“一帧一 fd”的对应关系，再包装成 tokio socket。非 Linux 平台
直接返回 `Unsupported`，宿主路径完全不变。

### 5.3 需要迁进 worker 的 VoWiFi socket

| socket | 旧位置 | worker 内绑定 |
|---|---|---|
| IKE/UDP 500/4500 | 宿主 WiFi 源地址 | **已迁移**：`bind_to_device=save<hex>`，默认路由走出 veth |
| SIP UDP/TCP（含 ipsec-3gpp 保护端口） | `SO_BINDTODEVICE(TUN)` | **已迁移**：`connect_sip_socket()` 按 UE context 走 worker，`bind_to_device=sa_vwf<hex>` |
| RTP/RTCP operator 侧 | `bind_with_operator_interface(TUN)` | **已迁移**：`RegisteredVoiceContext.media_operator_creator` → `bind_operator_relay()`，`bind_to_device=sa_vwf<hex>` |
| DNS | 宿主 `/etc/resolv.conf` | 后续移入 worker 后使用 UE 侧 DNS |

内部 leg（Asterisk/Trunk 侧，通常 `127.0.0.x`）**留在主进程**，不需要进 netns。

### 5.4 启用条件（配置）

```yaml
ue_isolation:
  enabled: true                 # 主开关：每个 UE 一个 netns + worker
  namespace_prefix: sa-ue
  host_veth_prefix: savh
  ue_veth_prefix: save
  veth_mtu: 1500
  vowifi_tun_in_namespace: true # stage-2b 门：TUN 进 netns 且 VoWiFi socket 走 worker
```

只有 `enabled && vowifi_tun_in_namespace` 同时为 true 时，线路注册表才会：

1. 创建 netns 并拉起 worker；
2. 创建 veth 对并让 worker 配置 UE 侧；
3. 注册 `line → (namespace, ue_veth_if, worker)`（socket context）；
4. 为 veth 宿主侧追加 MASQUERADE；
5. VoWiFi 下一次重连时 TUN 进 netns；IKE、SIP、RTP socket 全部通过 worker
   在 UE netns 内创建；
6. 关闭隔离或线路移除时，namespace 与 socket context 两个注册表都会被清理，
   下一个重连自动回到宿主路径。

任何一步失败都只告警并**回退到旧的宿主路径**（`None` 分支），不中断现有功能。

> 验收前提：阶段二 b 必须先在 410 上通过 §8 的“单 UE 验证清单”。通过之前不进入 VoLTE
> （§6.1），核心 `volte/bearer.rs`、`volte/live.rs` 注册链保持不动。

---

## 6. 后续阶段计划（VoLTE → 数据代理/Trunk → 5G）

### 6.1 阶段三：VoLTE

目标：IMS 注册所需的 socket 与 `wwanX` 都在同一个 UE netns 内，禁止跨线干扰。

已验证的前提：

- 空闲 `wwanN` 移入 netns 不干扰 ModemManager ⇒ 优先占用空闲通道；
- `wwan0` 移出会触发 MM 重建 Modem ⇒ 保留给默认数据，SimAdmin 需要容忍重新探测窗口。

实施选项：

1. **veth 桥接**：`wwanX` 留在宿主，主进程把 wwanX 的地址/路由通过 veth 转进 UE netns，
   UE netns 内 socket 绑定 veth UE 侧；改动小，但 P-CSCF 流量要过两层转发。
2. **`wwanX` 直接进 netns + `VolteSipChannel`/register 迁入 worker**：
   最彻底、隔离最干净，但 `volte/bearer.rs`、`volte/live.rs` 的注册与多媒体通道都要
   改造为 worker 通信，属于核心 IMS 重构。

结论：**优先方案 2 的“wwanX 进 netns”，配合 worker 内的 `VolteSipChannel`；**
在方案 2 的链路全部回归通过前，保留方案 1 作为过渡桥接。

状态：**未开始**（等待阶段二 b 实机验证通过后再动核心注册链）。

验收标准：

- 两台并发 UE 拿到相同 IP/网关/P-CSCF 时，VoLTE 均可独立注册、互不串扰；
- MM 重探测期间线路自动恢复；
- SMS over IMS、通话、DTMF 与现有宿主路径行为一致。

### 6.2 阶段四：数据代理与 Trunk 映射（未开始）

目标：HTTP/SOCKS5 代理的入口/出口一一映射到 UE：

```text
UE netns → per-UE proxy（监听 UE 侧地址）→ 宿主 → 对应 Modem/wwanX
```

实施要点：

- `data_proxy` 按 `UeContext.ue_id` 拆分实例，拒绝“共享代理 + 出口靠 IP 猜”；
- proxy 监听 socket 迁进 worker（复用 `UeSocketSpec`），出口绑定绑定到该 UE 的
  wwanX/veth；
- Trunk（SIP/RTP）的远端地址解析与媒体出口跟随 `UeContext`，不能共享 RTP relay 状态；
- 自动化/通知等按 line_id 归类的功能保持现有语义，只把网络出口改为 UE 确定。

### 6.3 阶段五：5G IMS（未开始，复用同一套 worker 模型）

- 5G 的 IMS 注册与 VoLTE 共享同一套 worker/注册链；
- worker 协议、socket 工厂、net-config 原样复用；
- 新增点只在 bearer/数据通道抽象（5G PDN/QoS 与 LTE 的差异），不新增隔离机制。

---

## 7. 硬件依赖与风险

### 7.1 还需在 410 验证的软硬件点

1. **veth + NAT 出 WiFi**：确认 `iptables`/`nft` 在自定义内核上的可用性；
2. **TUN fd 跨 netns 读写**：ESP 转发器从主进程读 UE netns 内的 TUN fd（Linux fd 语义上可行，
   需实测吞吐与延迟）；
3. **MM 对 wwan 移出的重探测**：SimAdmin 的重新 probe 逻辑要以实测为准；
4. **DNS**：UE netns 内 `/etc/resolv.conf` 或 worker 内 DNS 客户端；
5. **MTU**：veth 1500 + ESP-in-UDP 分片，与现有 `SIMADMIN_AUTO_FRAGMENT` 配合。

### 7.2 已知边界

- 本仓库在 Windows 上只能做 `cargo check`/单元测试；netns、setns、SCM_RIGHTS 必须上 410。
- `ue_isolation.enabled` 默认 false —— 未开启时行为与旧版完全一致。
- 阶段三、四、五均**未开始**；阶段一/二（worker 底座 + VoWiFi socket 迁移）代码已完成，
  `cargo check --all-targets` 通过，`ue_`/operator/media 相关测试通过（Windows 上有
  少量依赖 Linux `ip` 的既有测试无法运行）。

---

## 8. 单 UE 验证清单（410）

1. 开启 `ue_isolation.enabled=true` + `vowifi_tun_in_namespace=true`，重启 simadmin；
2. 确认日志：netns 创建、worker Hello、veth 配置成功；
3. `ip netns exec sa-ue<hex> ip addr`：能看到 `lo`、`save<hex>`、`sa_vwf<hex>`；
4. VoWiFi 连接：IKE 在 worker 内建立（日志中出现 worker socket 创建），
   SIP REGISTER 通过 TUN 完成，IMS 注册成功；
5. 收发短信经 VoWiFi；`tcpdump` 在 UE netns 内能看到 SIP/RTP 走 TUN；
6. 第二张 SIM 同时上线（相同 P-CSCF/IP 场景），确认互不干扰；
7. 飞行模式/退出 VoWiFi：TUN 与 worker 干净回收，无残留 `sa-ue*`、`savh*`、`sa_vwf*`。

全部通过后进入 VoLTE 阶段（§6.1）。
