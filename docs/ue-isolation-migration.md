# 多 UE 隔离架构迁移文档（Option B：per-UE worker + setns）

> 状态：**阶段一至四已完成代码实现，并已在 410 实机全量验证 —— 隔离四门中
> `enabled` / `vowifi_tun_in_namespace` / `three_gpp_ims_sockets_in_worker` /
> `data_proxy_in_worker` 全部打开时，VoWiFi、VoLTE 与蜂窝数据三条业务同时在线
> （2026-08-23 实机验证）；`trunk_sockets_in_worker` 因需 Asterisk 配置仍未验证；
> 阶段五已完成不改变现有硬件行为的通用模型底座**。
> 本文档记录当前代码状态、控制协议，以及 VoWiFi → VoLTE → 数据代理/Trunk → 5G
> 的迁移计划与验收标准。
>
> 2026-08-30 精简：删掉了逐轮实现笔记和带日期的回归记录（原 §4.1.1–4.1.4、
> §8.5–8.9，约 420 行）。那些是过程记录，git 历史里能查到，留在文档里只会让人
> 分不清"设计如此"和"某一轮临时结论"。§8.7 那 170 行 `smd_dsm_memcpy.c:297`
> 根因分析也删了——那是 `QCM410_BAM_DMUX_MODEM_CRASH.md` 的主题，那份讲得更完整。

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
- **worker 负责**：UE netns 内的 socket（IKE、SIP、RTP、代理出口等）、受限 XFRM
  操作与网络配置执行；SimAdmin 自建的 secondary/native `wwanX` 可迁入该 netns。
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
| `services/ue_worker.rs` | worker 进程管理、Hello 握手、`NetStatus`、`NetConfigRequest/Result` 关联批处理、`Ping/Pong`、优雅退出；socket 工厂、受限 `ip xfrm`、按线路/功能注册表已实现 |
| `services/ue_netcfg.rs` | 纯函数规划器：veth 地址/名称、UE 侧 ops、TUN ops、wwan ops（可单元测试） |
| `connectivity/.../vowifi/live.rs` | 每线路单一 **UE socket context 注册表**（同时持有 namespace、UE veth 与 worker）；TUN namespace 与 IKE/SIP/RTP socket 均从同一 context 派生，避免刷新时出现归属分裂 |
| `connectivity/.../vowifi/operator.rs` + `connectivity/core/media.rs` | **RTP/RTCP operator 侧 socket 通过 `OperatorSocketCreator` 走 worker**；Asterisk 内部 leg 仍留在宿主 |
| `connectivity/.../vowifi/tun_gateway.rs` | TUN 创建后 `ip link set ... netns <ns>`，netns 内配地址/路由；`None` 时代码保持旧宿主路径 |
| `services/line_registry.rs` | 线路刷新时 `reconcile_ue_context()`：ensure netns → spawn worker → veth → worker 应用 UE 侧配置 → 原子发布 socket context；普通读卡映射刷新不清它，关闭隔离/线路消失时清理；worker 不可用时还会同步清掉旧 TUN/SIP live runtime，防止宿主 fallback 复用 UE-only TUN |
| `platform/netns.rs` | `ensure_host_veth_nat()`：宿主侧 MASQUERADE（幂等检查后追加） |
| `connectivity/modems/ims/volte/{bearer,native_bearer,channel,pcscf,ipsec,live}.rs` | native IMS `wwanX` 受限迁移；worker 内 IP/路由、P-CSCF DNS、SIP、XFRM、RTP；失败清理与接口回宿主 |
| `hardware/devices/qcm410/secondary_qmi_data.rs` | DATA6/secondary QMI bearer 迁入对应线路 worker，停止时清理并把接口移回宿主 |
| `hardware/cellular/data_proxy.rs` | HTTP/SOCKS5 监听仍在宿主，出站 TCP socket 由对应线路 worker 创建并绑定该线路接口 |
| `platform/config.rs` | `ue_isolation` 配置块（见 §6） |

### 4.1.1 本阶段的实现细节

worker 生命周期、线路刷新发布一致性、worker 代次（generation）绑定和 socket 的
close-on-exec 都已落地，单元测试在 `services/ue_worker.rs` 内。具体的修复过程和
当时的取舍记录在 git 历史里，不在这里复述——需要时按上表的文件名查 blame。

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
- `link_set_mtu`、按设备 flush route、无显式网关的 WWAN default route
- 受限的 `xfrm state/policy add/delete/flush`（不接受任意 `ip` 子命令）

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
  three_gpp_ims_sockets_in_worker: false # stage-3：VoLTE/未来 VoNR 的 bearer、SIP、XFRM、RTP
  data_proxy_in_worker: false   # stage-4：数据代理出口 socket + secondary DATA bearer
  trunk_sockets_in_worker: false # stage-4：operator RTP socket；Asterisk/internal leg 留宿主
```

`trunk_sockets_in_worker` **依赖 `three_gpp_ims_sockets_in_worker`**：trunk 媒体只能
跟随一个已经进入 UE netns 的 bearer。只开 trunk gate 会宣告一个看不到 bearer 接口的
worker，RTP socket 要么绑定失败、要么沿一条含糊的宿主路由发出去——一种“看似已启用、
实际仍在宿主”的半迁移状态。因此配置侧提供
`UeIsolationConfig::effective_trunk_sockets_in_worker()` 作为唯一判定入口，缺少依赖时
发布的 feature 为 false 并告警一次（不按线路刷新重复刷屏）。
注意 `volte/live.rs` 中 `three_gpp_ims || trunk_sockets` 是有意的：bearer 一旦进入
worker，媒体 socket 必须跟随，即使 trunk gate 没开。

只有 `enabled && vowifi_tun_in_namespace` 同时为 true 时，线路注册表才会：

1. 创建 netns 并拉起 worker；
2. 创建 veth 对并让 worker 配置 UE 侧；
3. 注册 `line → (namespace, ue_veth_if, worker)`（socket context）；
4. 为 veth 宿主侧追加 MASQUERADE；
5. VoWiFi 下一次重连时 TUN 进 netns；IKE、SIP、RTP socket 全部通过 worker
   在 UE netns 内创建；
6. 关闭隔离、线路移除或 worker 不可用时，单一 socket context 被清理；namespace
   查询也随之返回 `None`，下一个重连自动回到宿主路径。

任何一步失败都只告警并**回退到旧的宿主路径**（`None` 分支），不中断现有功能。

回退不再是“只发布 `None`”：egress 准备过程可能已经建好 worker、veth、NAT 规则和
namespace，只把 socket context 置空会把这些资源留成孤儿，并让一个仍在运行的 worker
继续持有 DATA 接口。现在失败路径执行**完整 teardown**，顺序固定为：

1. `secondary_data.stop()`——此时 worker 控制通道仍然可用，能先删掉 netns 内的
   地址与路由，再把接口移回宿主；
2. `worker.shutdown()`——让 namespace 里不再有运行中的进程；
3. `teardown_ue_isolation_locked()`——注销 worker/socket context 注册表、删除 NAT、
   拆除 veth、移除 namespace、清空 egress fingerprint。

“先停 DATA、再停 worker”这一顺序与线路消失路径、关闭隔离路径现已完全一致；
反过来先杀 worker 会让 DATA 的 netns 内清理指令发不出去。下一次线路刷新会重新
`ensure_netns` + `spawn`，因此该路径是自愈的。

四个功能门默认均为 `false`。虽然阶段三/四代码已经完成，410 回归仍必须按
VoWiFi → VoLTE → 数据代理/Trunk 的顺序逐个启用；不能一次打开全部功能后把故障归因给
任意一层。

---

## 6. 阶段三至五状态（VoLTE → 数据代理/Trunk → 5G）

### 6.1 阶段三：VoLTE

目标：IMS 注册所需的 socket 与 `wwanX` 都在同一个 UE netns 内，禁止跨线干扰。

已验证的前提：

- 空闲 `wwanN` 移入 netns 不干扰 ModemManager ⇒ 优先占用空闲通道；
- `wwan0` 移出会触发 MM 重建 Modem ⇒ 保留给默认数据，SimAdmin 需要容忍重新探测窗口。

已实现：

- 仅把 SimAdmin 本次 native QMI IMS session 自己创建的 secondary 接口迁入 worker；
  明确拒绝迁移 ModemManager 主接口；
- 迁移判定已改为读取 bearer provider 的 `interface_ownership`：QCM410 DATA6
  标记为 `sim_admin_owned_secondary`，`host_managed_primary` 和 `unknown` 均拒绝迁移，
  不再把接口名称当作所有权依据；
- worker 内配置 IMS 地址、MTU、P-CSCF/DNS/媒体路由；
- P-CSCF DNS、`VolteSipChannel`、Security-Agree 端口、XFRM、音频/视频 RTP socket
  均可在对应线路 worker 内创建；
- 注册失败/停止时按设备清理路由与地址、卸载本次 XFRM，并把 native 接口移回宿主；
- worker 不可用、接口迁移或网络配置失败时保留/恢复宿主路径；ModemManager bearer
  仍使用原宿主实现，不会迁移其活动接口。

状态：**代码完成，410 实机回归待执行**。当前没有采用 veth 桥接 ModemManager
`wwan0`；这是一条有意保留的安全边界，不应在未验证硬件行为前扩大迁移范围。

验收标准：

- 两台并发 UE 拿到相同 IP/网关/P-CSCF 时，VoLTE 均可独立注册、互不串扰；
- MM 重探测期间线路自动恢复；
- SMS over IMS、通话、DTMF 与现有宿主路径行为一致。

### 6.2 阶段四：数据代理与 Trunk 映射

目标：HTTP/SOCKS5 代理的入口/出口一一映射到 UE：

```text
UE netns → per-UE proxy（监听 UE 侧地址）→ 宿主 → 对应 Modem/wwanX
```

已实现：

- `data_proxy` runtime 继续按线路持有；HTTP/SOCKS5 listener 留在宿主以保持入口兼容，
  每条连接的 outbound TCP socket 由该线路 worker 创建并 `SO_BINDTODEVICE`；
- qcm410 secondary DATA/DATA6 bearer 可迁入该线路 worker；停止时只清理该接口并移回宿主；
- retained DATA session 复用前会同时验证 QMI CID、当前 worker generation 和 worker 内接口快照；
  worker 崩溃/namespace 重建后不会继续复用悬空接口，而是先释放旧 session 再重新建立；
- 线路移除、禁用隔离或 worker reconcile 失败时，会先停止 secondary DATA，再拆除 worker/veth/namespace；
- Trunk/operator RTP socket 可由线路 worker 创建；Asterisk/internal leg 留在宿主，
  dialog、自动化和通知继续按 `line_id` 归属；
- 接口与 worker 的选择来自线路注册表，不再以运营商分配的 IP 推断 UE，因此不同 UE
  使用相同 IP/网关时仍有独立网络栈。

状态：**代码完成，410 数据代理与 Trunk 媒体回归待执行**。worker 内 DNS 仅在
VoLTE P-CSCF 查询中实现；普通代理域名当前仍由宿主解析，再由 worker 建立数据连接。

### 6.3 阶段五：5G IMS（通用数据模型接口完成，NR 硬件适配未开始）

- 3GPP worker 配置和 feature 已采用 LTE/NR 通用命名，worker 协议、socket 工厂、
  XFRM、RTP 与网络配置可直接复用；
- bearer 抽象现在可以携带实际 RAT（LTE/NR NSA/NR SA）、EPS/5GS 域、PDU
  session、QoS flow 和接口归属；这些字段只由硬件 provider 提供，缺失时保持
  `unknown`，不会从“支持 5G”或 NR 小区信息推断 VoNR 已就绪；
- 当前 native bearer provider 仍只实现 QCM410 DATA6/QMI 路径，尚未完成按设备能力
  选择 provider 的 factory，也没有 ModemManager/MBIM/QMI 5GS provider；因此运行时
  目前不会产生真实的 5GS PDU session 或 QoS flow 数据；
- 尚未实现 NR 专用 bearer 建立、5G QoS flow/PDU session、VoNR 能力探测和硬件适配；
- 新增 NR 支持时应扩展 bearer/数据通道抽象，不再创建第二套 namespace 隔离机制。

---

## 7. 硬件依赖与风险

### 7.1 还需在 410 验证的软硬件点

1. **veth + NAT 出 WiFi**：确认 `iptables`/`nft` 在自定义内核上的可用性；
2. **TUN fd 跨 netns 读写**：ESP 转发器从主进程读 UE netns 内的 TUN fd（Linux fd 语义上可行，
   需实测吞吐与延迟）；
3. **MM 对 wwan 移出的重探测**：SimAdmin 的重新 probe 逻辑要以实测为准；
4. **DNS**：VoLTE P-CSCF 已使用 worker UDP DNS socket；VoWiFi/普通数据代理的其余
   域名解析仍需按实际运营商行为验证是否必须 per-UE；
5. **MTU**：veth 1500 + ESP-in-UDP 分片，与现有 `SIMADMIN_AUTO_FRAGMENT` 配合。

### 7.2 已知边界

- 本仓库在 Windows 上只能做 `cargo check`/单元测试；netns、setns、SCM_RIGHTS 必须上 410。
- `ue_isolation.enabled` 默认 false —— 未开启时行为与旧版完全一致。
- 阶段一至四代码已完成但 feature gate 默认关闭；未经 410 回归不能宣称生产可用。
- 阶段五目前只完成通用模型和投影，尚不代表已经支持 VoNR；需要支持 5G 的硬件
  provider 真正填充 bearer/PDU/QoS 字段后，再单独实现并验收 NR bearer。
- Windows 与 WSL Linux 的 `cargo check --all-targets` 已通过；Windows 上的 Unix/netns
  路径由条件编译替代实现覆盖，真实 `setns`、SCM_RIGHTS、XFRM 必须上 Linux/410 验证。

---

## 8. 分阶段实机验证清单（410）

### 8.1 worker 与 VoWiFi

- [x] 只开启 `enabled + vowifi_tun_in_namespace`，确认 netns、worker Hello、veth/NAT 成功；
- [x] worker 空闲超过 60 秒仍保持 `ready=true`，Ping/Pong 可见，控制通道不被误关闭；
      （2026-08-22 / 9ae297a：静置 150 秒跨两个 60s 读超时，worker pid 不变，控制通道错误 0 条）
- [x] worker 进程异常退出后，状态清除旧 PID，下一次 reconcile 自动重启且无 zombie；
      （2026-08-22 / 9ae297a：连续 4 次 `kill -9`，每次约 5 秒拉起替代进程，zombie 恒为 0，
      netns/veth/NAT 始终各 1 份，net-config 每次重新下发）
- [x] worker 内可见 `lo`、`save<hex>`、`sa_vwf<hex>`；IKE、TUN、SIP、RTP
  均属于当前线路 namespace；
      （2026-08-22 / 9ae297a：TUN 取得运营商地址与 IPv6，两条 P-CSCF
      主机路由均在 netns 内；IKE `0.0.0.0:500` socket 在 netns 内、fd 由主进程持有；
      宿主侧无 `sa_vwf` 泄漏；IKEv2/EAP-AKA/ESP 隧道建立成功）
- [x] VoWiFi IMS REGISTER 成功（2026-08-22 / dd4bb0f）：
      `status_code=200 auth_rounds=1 expires_seconds=3600`，随后
      `Voice over IMS signaling readiness validated preferred_codec="amr-wb"`；
      受保护的 ipsec-3gpp 两条流（5063↔7807、5064↔7777）均在 UE netns 内 ESTAB。
- [ ] VoWiFi 短信、来电/接听、双向 RTP、DTMF、挂断同步正常；（signaling 已就绪，待拨打测试）
- [ ] 飞行模式下仍可用 VoWiFi，退出后 TUN/XFRM/路由均无残留；
- [ ] P-CSCF 或 worker socket 失败时错误可观测，worker 崩溃后线路能恢复。

### 8.2 VoLTE

- [ ] 再开启 `three_gpp_ims_sockets_in_worker`，确认 `wwan0` 始终留在宿主；
- [ ] SimAdmin native `wwanN` 迁入正确 worker，地址、MTU、DNS/P-CSCF route 正确；
- [ ] P-CSCF PCO、AT fallback、worker DNS fallback 分别验证；
- [ ] Security-Agree/XFRM 安装在对应 worker，注销/失败后只删除本 session 项；
- [ ] VoLTE 短信、来电/接听、双向 RTP、DTMF、视频协商/降级与挂断同步正常；
- [ ] 迁移、网络配置或 worker 失败时接口回宿主且 native WDS session 可干净释放；
- [ ] 线路停止后 native `wwanN` 回到宿主，不遗留地址、route、XFRM 或 QMI client。

### 8.3 数据代理与 Trunk

- [ ] 开启 `data_proxy_in_worker`，DATA6/secondary bearer 迁入正确 worker；
      **被硬件门阻塞**：DATA6 需 `SIMADMIN_ENABLE_SECONDARY_QMI=1`，而该开关被标注为
      「410 固件把 AT 端点强制以 QMI 打开时可能导致 modem 崩溃」，默认关闭。2026-08-22
      回归时明确选择不启用，因此本节数据代理各项均未实机验证。
- [ ] worker 异常退出并重建后，旧 DATA session 不被复用，接口能重新建立并迁入新 worker；
      （代码侧已由 worker generation 绑定保证，见 §4.1.1，并有单元测试
      `binding_detects_a_respawned_worker_behind_the_same_handle`；实机部分随 DATA6 一起阻塞）
- [x] egress 配置失败后，`ip netns list`、`ip link`、`iptables -t nat -S` 均无残留的
      namespace / veth / MASQUERADE，且没有游离的 `ue-worker` 进程；
      （2026-08-22 / 9ae297a：注入非法 `veth_mtu=999999` 使 `ensure_veth_pair_host_side`
      失败后，netns/veth/NAT/worker 计数全部归 0，zombie 0，服务未崩溃；改回 1500 后
      自动重建且各资源仍各 1 份）
- [ ] HTTP CONNECT、普通 HTTP、SOCKS5 的 DNS/IP 目标都从对应线路出站；
- [ ] 停止代理/数据连接后接口回宿主，worker 内只清理该接口路由；
- [ ] 开启 `trunk_sockets_in_worker`，operator RTP 属于当前线路 worker，Asterisk leg
  仍在宿主且双向媒体正常；
- [x] 只开 `trunk_sockets_in_worker` 而不开 `three_gpp_ims_sockets_in_worker` 时，
  日志出现一次抑制告警，operator RTP 仍留在宿主（不得出现半迁移）；
      （2026-08-22 / 9ae297a：窗口内发生 4 次 reconcile（4 次 worker ready + 4 次 egress
      veth configured），抑制告警恰好 1 条，证明 `Once` 生效且不随刷新刷屏）
- [ ] Trunk 注册、来电、挂断和转发规则仍只作用于当前线路。

### 8.4 多线路强隔离

- [ ] 两个 UE 同时获得相同 UE IP、网关、P-CSCF、RTP 对端时仍能分别注册和传输；
      （410 上目前只插了 1 张卡，`line_profiles` 只有一条线路，无法验证）
- [ ] 每条线路的 SIP dialog、XFRM、RTP、数据代理计数与 Trunk 映射不串线；
- [ ] 单个 worker 崩溃、线路断开或 bearer 重连不破坏另一条线路；
- [x] 重启 SimAdmin 后稳定命名能够回收旧资源，不生成重复 namespace/veth/NAT 规则。
      （2026-08-22 / 9ae297a：本轮回归共重启服务 6 次、重启 worker 5 次，
      `sa-ue286e0c9d2870` / `savh0c9d2870` / MASQUERADE 规则始终各 1 份，无累积）

