# 多 UE 隔离架构迁移文档（Option B：per-UE worker + setns）

> 状态：**阶段一至四已完成代码实现，并已在 410 实机全量验证 —— 隔离四门中
> `enabled` / `vowifi_tun_in_namespace` / `three_gpp_ims_sockets_in_worker` /
> `data_proxy_in_worker` 全部打开时，VoWiFi、VoLTE 与蜂窝数据三条业务同时在线
> （2026-08-23，见 §8.8）；`trunk_sockets_in_worker` 因需 Asterisk 配置仍未验证；
> 阶段五已完成不改变现有硬件行为的通用模型底座**。
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

### 4.1.1 worker 生命周期修复（本轮）

实机日志暴露出一个会影响所有隔离功能的边界问题：父进程 reader 在控制通道
连续空闲 60 秒时，把 `poll(2)` 超时误当作通道关闭，worker 随后退出；旧 PID
还留在状态快照中，线路刷新也不会重新拉起 worker。结果是后续 IKE/SIP/RTP
请求统一报 `worker control channel is not up`。

现已修复：

- 空闲超时发送 `Ping`，只有真正 EOF 或 I/O 错误才关闭控制通道；
- 控制通道关闭时立即失败所有未完成的 net-config/socket 请求；
- 回收旧 child、清除 PID，下一次线路 reconcile 可自动重启 worker；
- 增加 Unix 控制帧的“空闲超时”和“真实 EOF”回归测试。

这项修复必须先在 410 上验证 worker 能持续运行超过 60 秒，再判断 IMS
注册、媒体或数据代理本身是否有问题。

### 4.1.2 线路刷新发布一致性（本轮）

线路刷新现在采用“准备 → 发布”两阶段流程。发现、namespace/veth 配置、worker
重启和网络检查期间，SIM/PCSC 映射、worker 注册表和 VoWiFi socket context 都
保持在私有快照中；只有对应的 `ModemBinding` 完成替换（或新线路插入）后，三者
才一起发布。线路消失时则先撤销这些全局映射，再标记线路离线，最后清理 worker、
veth 和 namespace。这样热插拔或 worker 重建不会让消费者把旧 binding 与新 UE
网络状态拼成一个短暂的撕裂快照。

数据代理和 secondary DATA6 复用同一生命周期边界：保留的 bearer 会核对当前
worker 实例及其 netdev 快照；worker 重启、namespace 重建或接口不可见时，旧会话
不会继续被当作可用出口，停止流程也会清理可能残留在宿主的地址/策略。

### 4.1.3 worker 代次（generation）绑定（本轮）

上一轮的“核对当前 worker 实例”实际上无法生效。`UeWorkerHandle` 在
`LineRuntime` 构造时创建一次，`spawn()`/`shutdown()` 都在**同一个**
`Arc<WorkerCore>` 上原地操作，因此 `same_instance()`（`Arc::ptr_eq`）在一条线路
的整个生命周期内恒为 true。worker 崩溃后重建，数据代理和 retained DATA session
仍然认为自己绑定的是当前 worker，§8.3 中“worker 异常退出并重建后，旧 DATA
session 不被复用”这一条并未真正被满足。

现已修复：

- `WorkerCore` 增加单调递增的 `generation`，每次成功 `spawn` 自增（0 表示尚未启动）；
- 新增 `UeWorkerBinding`：把 handle 与**捕获时刻的代次**一起保存。
  `is_current()` 判断绑定的进程是否仍在运行，`matches()` 同时比较线路 worker 与代次；
- `DataProxyRuntime` 的监听器、`SecondaryDataSession` 均改为持有 `UeWorkerBinding`；
  代次不一致时，代理会被重建、retained DATA session 会先释放再重新建立；
- `stop_session()` 只在 `is_current()` 时通过控制通道下发地址/路由清理，
  避免把清理指令发给一个从未配置过该接口的替代 worker；接口回宿主与宿主侧
  地址/策略清理仍然无条件执行；
- `same_instance()` 语义收敛为“同一条线路的 worker 管理器”，其文档明确指出
  它不区分进程代次。

单元测试 `binding_detects_a_respawned_worker_behind_the_same_handle` 直接构造
“同一 handle、代次自增”的场景，锁定这个回归。

### 4.1.4 worker socket 的 close-on-exec（本轮）

实机拿到 IMS 注册之后，一次 worker 重启暴露出 fd 归属问题：

```text
ESTAB 2.194.56.78:5063 ... users:(("simadmin",pid=155950,fd=23),
                                  ("simadmin",pid=150999,fd=23))
```

`pid=150999` 是通过 `SCM_RIGHTS` 收下 fd 的父进程，`pid=155950` 则是很久之后才
拉起的替代 worker。`recv_control_frame` 调用 `recvmsg` 时没有传任何 flag，因此
父进程收下的每个 fd 都不带 close-on-exec，会被后续每一次 fork+exec 继承。

这恰好破坏了本次迁移要保证的 socket 生命周期：父进程关掉某个 IMS / RTP /
代理 socket 时并不会真正释放它，因为一个从未创建过它的 worker 仍持有引用，
已经退役的注册可能长期停留在 ESTAB；同时每次 worker 重启都会多泄漏一组 fd。

修复：接收侧 `recvmsg` 传 `MSG_CMSG_CLOEXEC`（Linux-only，其余 Unix 保持原样
以免编译失败）。

实机验证（2026-08-22 / ab6177a）：注册就绪后 `kill -9` worker，替代进程
**没有**出现在 IMS socket 的 owner 列表中，新 worker 仅 11 个 fd，zombie 为 0，
注册本身不受影响。

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
- [x] VoWiFi IMS REGISTER 成功（2026-08-22 / dd4bb0f，见 §8.6）：
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
      （代码侧已由 worker generation 绑定保证，见 §4.1.3，并有单元测试
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

### 8.5 2026-08-22 回归结论（build 9ae297a）

设备：`192.168.100.13`，Debian 13 aarch64，内核 `6.17.0-rc6-lkiuyu-compile+`，
Modem 注册在 MY MAXIS（50212）LTE，信号 86%。

已验证通过：

- worker 生命周期（异常退出重建、长时间空闲、控制通道不误关）；
- egress 失败的完整 teardown 与自愈；
- trunk 门依赖与一次性抑制告警；
- VoWiFi 数据面完全落在 UE netns 内（TUN、运营商地址、P-CSCF 路由、IKE socket），
  宿主无泄漏；这同时验证了 §7.1 的第 1、2 项（veth+NAT 出 WiFi 可用、
  主进程跨 netns 读写 TUN/socket fd 可用，`iptables` 在该自定义内核上可用）。

未通过或阻塞：

- **VoWiFi IMS REGISTER 已修复并通过**（见 §8.6）：200 OK，voice signaling 就绪，
  可以进行实机通话/短信测试。
- **VoLTE（§8.2）根因已定位到设备侧，且状态已显著改善**，见 §8.7。
- **数据代理（§8.3 大部分）阻塞**：需 `SIMADMIN_ENABLE_SECONDARY_QMI=1`，
  存在固件崩溃风险，本轮决定不启用。
- **多线路（§8.4）**：只有一张卡，无法验证跨线路互不干扰。
- 通话与短信按要求不在本轮范围内。

### 8.6 VoWiFi IMS REGISTER 421/400 修复（2026-08-22）

回归初期 VoWiFi REGISTER 一直被 Maxis 以 `421 Extension Required` 拒绝。抓包
（在 UE netns 内对 TUN 抓明文，外层 ESP 由用户态终结）给出了完整证据链，并且
**先用 `ue_isolation.enabled=false` 做了 A/B**：宿主路径发出的 REGISTER 与隔离
路径逐字段一致、同样被回 421，因此该问题与 UE 迁移无关（相关默认值比迁移早 12 天）。

三个连续的缺陷，每个都由抓包确认后单独修复：

| 请求形态 | 运营商响应 | 缺陷 |
|---|---|---|
| `Security-Client`，无 `Require` | `421 Require: sec-agree` | `auto` 模式下没有任何变体会声明 sec-agree |
| `+ Require: sec-agree` | `400 Bad Request` | 缺少配套的 `Proxy-Require`（RFC 3329 §2.3） |
| `+ Proxy-Require: sec-agree` | `400 Bad Request` | 缺少 IMS AKA 的空 `Authorization`（TS 24.229 §5.1.1.2.2） |
| 三者齐备 | **`401` → AKA → `200 OK`** | — |

根因是 `security_agreement` 的 `auto` 兜底不完整：从真机提取的 bundle 通常不写
这个字段，落到 `auto` 后 `include_security_client` 为 true 而
`force_sec_agree_headers` 为 false，且另一个变体 `catalog_v7_challenge_first`
干脆不带 Security-Client —— 没有任何一条路径能发出运营商要求的声明。

修复方式刻意**不是**"只要带 Security-Client 就强制加 Require"：`GB_EE_23433`
是真机抓取的形态，故意只带 `Supported: sec-agree` 而不带 `Require`，其单元测试
锁定了这一点。改为按运营商实际要求自适应（与 VoLTE 侧既有做法一致）：

- 把 421 中的 `Require: sec-agree` 需求经 `LiveStageError` 带出；
- 用同一变体加上 Require/Proxy-Require 重试一次，保留该运营商已验证的
  request-URI、authorization 与 Security-Client 格式；
- catalog 侧 `proxy_require_sec_agree_headers` 与 `initial_authorization`
  改为跟随"是否真的提供了 Security-Client"。`profiles.rs` 中的硬编码 profile
  不走 catalog 路径，因此 EE / Vodafone 的既有形态不受影响。

注意：认证轮的 REGISTER 实测 `inner_packet_bytes=1352`，而软件分片阈值
`AUTO_FRAGMENT_INNER_IP_MAX` 为 1356 —— **只剩 4 字节余量**。再增加任何头部
（PANI、sip.instance、更多 Contact 参数）都会跨过阈值并触发分片路径，该路径
目前尚未在实机上被真正走过，需要单独验证。

### 8.7 VoLTE：`smd_dsm_memcpy.c:297` 的根因定位（2026-08-22）

原始症状：`volte_bearer_netdev_runtime_error:interface=wwan0:
runtime_status=error before OPEN`，wwan0..7 全部无法 OPEN。

**根因不在 SimAdmin。** 每次冷启动 modem 固件都会崩一次
`qcom-q6v5-mss 4080000.remoteproc: fatal error received: smd_dsm_memcpy.c:297`
（DSM = Data Services Memory）。remoteproc 自身恢复为 `running`，但 Linux 侧
`bam-dmux` 驱动的 runtime-PM 被锁存在 `error`，所有 wwan netdev 继承该状态。

按顺序做过的排查，每一步都排除了一类原因：

| 实验 | 结果 | 排除了什么 |
|---|---|---|
| 手工 `ip link set wwan1 up` | `RTNETLINK: Invalid argument` | SimAdmin 的前置检查是**正确**的，不是过严 |
| 热重启 modem 子系统（stop/start remoteproc） | mpss 干净加载，**不崩** | **MPSS 镜像本身没问题**（它能通过签名并正常运行） |
| unbind/rebind `bam-dmux` | `error` → `suspended`，但 netdev 全消失，报 `Timed out waiting for remote side to suspend` | `error` 是 **Linux 驱动侧锁存状态**，重启固件清不掉 |
| 恢复出厂 `modemst1/modemst2`（fastboot） | 崩溃照旧，**IMEI 与校准完好** | **排除 EFS/NV 损坏**；顺带证明该操作无害 |
| 拉黑 `qcom_bam_dmux` 后冷启动 | **完全不崩** | **mainline `qcom_bam_dmux` 的 probe 就是冷启动崩溃的触发点** |
| 延迟到 modem 稳定后手工 `modprobe` | 不崩，8 个 netdev 建出，`wwan1` 能 UP | 延迟加载可规避该竞态 |

固件是 2022-11-05，内核是 6.17.0-rc6 mainline，中间隔了三年，而 mainline 的
`qcom_bam_dmux` 是社区重新实现的驱动 —— 这是典型的新驱动 × 旧厂商固件时序竞态。

**重要推论：重刷同一个系统镜像不会修复它**，因为内核与固件都在镜像里，冷启动
竞态会照样复现。只有厂商固件/内核升级，或在启动时规避该竞态，才有意义。

恢复出厂 EFS 之后（虽然没有阻止崩溃）崩溃时刻从 t≈15.8s 推迟到 t≈38s，
**晚于 bam-dmux 建立通道的时刻**，于是驱动扛住了：`bam-dmux runtime_status`
变为 `suspended`，8 个 netdev 正常，`wwan1` 可以 UP，ModemManager 报
`state: connected`。VoLTE 因此前进到能建立 bearer 并真正发出 IMS REGISTER
（`request_bytes=981`）。

当前 VoLTE 的失败点已经**不是**接口问题，而是更下层的 **IMS PDN 根本没有激活**。
向 modem 直接查询（经 ModemManager 的 AT passthrough，只读）：

```text
AT+CGDCONT?      +CGDCONT: 2,"IPV4V6","ims","0.0.0.0",0,0   ← IMS context 已定义
AT+CGPADDR       +CGPADDR: 2,0.0.0.0                        ← 但没有分到地址
AT+CGCONTRDP=2   (空)                                        ← 没有任何动态参数

ModemManager Bearer/2 (apn: ims):
  connected: no
  connection error: "Call failed: cm error: client-end"   attempts: 3
```

因果链因此是：**IMS PDN 激活失败 → 没有已激活的 IMS context → PCO 自然下发不了
P-CSCF → 代码回退去读 IMS context，拿到的是 VoWiFi 侧的 `172.20.x` 地址 →
这些地址在 LTE bearer 上不可达 → `ims_register_initial_receive_failed`。**

路由层面还观察到一个真实缺陷：SimAdmin 的策略表 `14002` 只为其中一个 P-CSCF
装了路由：

```text
ip route show table 14002
  2.242.195.192/26  dev wwan1 scope link
  172.20.225.221    dev wwan1 scope link        ← 只有这一个

ip route get 172.20.225.221 from 2.242.195.224 → dev wwan1 table 14002   正确
ip route get 172.20.58.221  from 2.242.195.224 → via 192.168.100.1 dev wlan0  **漏到 WiFi**
```

即候选 P-CSCF 列表里没装路由的那个会经由宿主 WiFi 发出去。即使 IMS PDN 修好，
这一条也应当修：要么为所有候选 P-CSCF 都装策略路由，要么在没有路由时直接跳过该
候选，而不是让它走默认路由泄漏到别的接口。

**下一步应从 IMS PDN 激活查起**（`cm error: client-end` 的具体原因、IMS APN 参数、
是否需要单独的 IMS profile / 鉴权），而不是继续在 SIP 或 netdev 层排查。

#### 8.7.1 已排除的三个假设（2026-08-23）

沿着上面的线索又排除了三条，记下来避免重复走：

**① 策略路由的源地址竞态 —— 不成立。** 曾观察到 `ip rule from <旧地址>` 与
`wwan1` 当前地址不符，怀疑是配置快照与发包时刻之间的地址竞态。为此加了
`volte_bearer_address_changed` 前置检查（`bearer.rs::interface_still_holds_address`），
实机跑下来**触发 0 次** —— 发 REGISTER 那一刻接口地址与策略路由是一致的。
之前那次"关联"取的是最新日志行 + 随后采样的路由，中间跨了 bearer 周期，不严密。
该检查作为防御保留，它把这类竞态从静默超时变成显式错误。

**② SIP 漏到 WiFi —— 不成立。** 在 `wwan1` 与 `wlan0` 上同时抓包：

```text
VoLTE local_port=56934
HOST  socket : ESTAB 2.181.181.169:56934 → 172.20.110.221:5060
NETNS socket : 无（socket 正确留在宿主，wwan1 本就是宿主接口）
wwan1 抓到   : 2 条 REGISTER
wlan0 抓到   : 0 条
```

包确实从 IMS bearer 出去、源地址正确、没有泄漏。**路由与源地址选择都是对的。**

**③ 缺少 sec-agree 声明 —— 不成立。** VoLTE 发的确实是 VoWiFi 侧被 421 拒绝的
那个不合规形态（`sec_agree_required=false`），而且因为 Maxis 在 LTE leg 上是
**直接丢弃而不是回 421**，原有的 421 升级阶梯永远触发不了。为此补了一级
`sec_agree_timeout_retry_variant`：初始 REGISTER 完全无响应时，用同一变体带上
`Require`/`Proxy-Require` 重试一次。实机确认该升级**已生效**：

```text
ATTEMPT variant=catalog_v7                             require=false proxy=true → 无响应
ATTEMPT variant=..._aka_uri_first_sec_agree_required   require=true  proxy=true → 无响应
```

**带齐全部 sec-agree 头之后运营商仍然完全不回**，所以 sec-agree 不是 VoLTE 的
阻塞原因。这一级阶梯本身是真实缺口，值得保留（对会回 421 的运营商有用）。

**④ 包过大 / 未触发分片 —— 不成立。** VoWiFi 那套软件分片
（`tun_gateway.rs`，`AUTO_FRAGMENT_INNER_IP_MAX=1356`）只作用于 TUN 路径；VoLTE 的
SIP 直接从 `wwan1` 这个 raw-IP 接口出去，不经过 TUN，因此完全不涉及那套机制。
在 `wwan1` 上按 IP 头逐包核对：

```text
#1 ip_total=1010  DF=True MF=False frag_off=0  captured_payload=982   单个完整数据报
#2 ip_total=1031  DF=True MF=False frag_off=0  captured_payload=1003  单个完整数据报
发往 P-CSCF:5060 : 2 个包        从 P-CSCF 回来 : 0 个包
```

`wwan1` MTU 为 1500，而 REGISTER 连同 IP+UDP 头只有 1010–1031 字节，既不需要分片
也确实没有分片（`MF=0`、`frag_off=0`）；抓到的载荷长度与 SimAdmin 自报的
`request_bytes` 逐字节吻合，没有截断。此外从同一 bearer 源地址发出的约 120 字节
极小 SIP OPTIONS 同样**没有任何回应** —— 如果是尺寸导致运营商收不全，小包应当能通。
因此报文大小与完整性都不是变量。（注：P-CSCF 过滤 ICMP，连 64 字节 DF ping 都不回，
所以无法用 ping 做路径 MTU 探测，这一点不能反过来当作 MTU 有问题的证据。）

#### 8.7.2 IMS bearer 不通，但普通数据 APN 是好的（2026-08-23）

> **本节曾经写错过，现已更正。** 早先的版本据此断言"整个蜂窝用户面已死、一切归因
> 于 DSM 固件崩溃"，那个结论是**错的**，由两个无效的测量得出，记录在此以免重犯。

**失效测量 ①：只绑源地址，没绑接口。** 之前用 `ping -I <地址>` 和
`socket.bind((src,0))` 做探测。主路由表里 `wlan0` 默认路由 metric 600 优先于
`wwan0` 的 700，所以那些包**带着蜂窝源地址从 WiFi 发了出去**，被当作非法源丢弃。
正确做法是绑接口（`SO_BINDTODEVICE`，`ping -I <ifname>`）。

**失效测量 ②：`tx_packets` 计数器。** bam-dmux 驱动**根本不更新** netdev 统计：
数据明明在正常收发，`/sys/class/net/wwan0/statistics/tx_packets` 依然是 0。这个 0
不能作为任何证据。（`QCM410_BAM_DMUX_MODEM_CRASH.md` §7 里那段"看 tx_packets 判定"
也随之作废，已一并更正。）

用正确方法重测后的**真实结论**：

| 接口 | ping 8.8.8.8 | ping 运营商 DNS 58.71.136.20 | SIP OPTIONS 到 P-CSCF |
|---|---|---|---|
| `wwan0`（数据 APN） | 2/3，16–38ms | 3/3，0% 丢包 | — |
| `wwan1`（IMS APN） | — | **100% 丢包** | **无回应** |

**普通数据 APN 完全正常，IMS APN 一个包都过不去。** 所以问题精确定位在 IMS bearer
本身，既不是整个用户面，也不是 SIP 内容、路由或分片。

#### 8.7.3 与迁移前版本的 A/B：不是 UE 架构的回归（2026-08-23）

把 UE 架构之前的构建（`551b6f8`，2026-08-19）部署回设备做对照。该版本用严格
反序列化，不认识 `ue_isolation` 字段而拒绝启动，因此测试期间临时移除了该键
（配置已先备份到 `/opt/simadmin/rollback-backup/`）。

结果：

- **VoLTE 失败方式完全一致** —— `ims_register_initial_receive_failed`，
  REGISTER 发出后 8 秒无响应，三次尝试后 `volte_runtime_all_pcscf_failed`；
- 同一时刻 `ping -I wwan1 58.71.136.20` 仍然 100% 丢包，而 `ping -I wwan0` 正常；
- **VoWiFi 在旧版本上是坏的**：反复 `registration_loss="network_rejected"`，
  因为它没有 §8.6 的 sec-agree 修复。这反过来确认了那几个修复是必需的。

**结论：VoLTE 的失败与 UE 隔离迁移无关**，迁移前后表现一致，问题在设备/运营商侧的
IMS 承载。同时新版本严格优于旧版本（VoWiFi 只在新版本上能注册），不应为了 VoLTE
回退。

按此判断，下一步是**整机重刷系统**验证是否为设备状态问题；若重刷后 IMS APN 仍然
不通，则应向运营商核实该 SIM 的 IMS 承载是否被授权给外部协议栈使用。

### 8.8 全隔离 + 三业务并存实机验证（2026-08-23，重刷后）

设备整机重刷、按最小集重新安装 SimAdmin（**只装二进制、前端、carrier catalog 和主
systemd unit，不装 `install.sh` 里的内核模块与 DATA6 udev/service**），随后按门逐级
打开隔离。全过程 `dmesg | grep -c 'fatal error received'` 恒为 **0**。

**先决修复**（本轮定位并修掉的两个真问题）：

- `f3308ed` —— `purge_legacy_rpmsg_module()`：按 beta8 的做法 `rmmod` + 删 `.ko` +
  `depmod -a`，并放在 DATA6 开关**之前**执行。旧代码只解绑单个设备，模块仍加载着，
  每次开机继续自动绑其余 `DATA*_CNTL`，撞崩正在初始化的 DSM（见
  `QCM410_BAM_DMUX_MODEM_CRASH.md` §8）。
- `f79c97d` —— `teardown_ue_isolation_locked()` 曾**无条件** `secondary_data.stop()`。
  隔离关闭时每次线路刷新都走该分支，把 watchdog 刚建好的宿主数据 bearer 在 98ms 后
  拆掉，循环往复 —— 这是数据一直不通的直接原因。改为仅在会话确实迁入 worker
  namespace 时才停。

**DATA6 与 IMS 可以并存**，槽位分配为
`mode="secondary_qmi_data"`、`allocation="IMS allocated to primary qmi0; DATA6 is
reserved for data"`。此前"DATA6 会崩固件"的判断是被残留的 `_multi` 模块污染的结论，
本轮全程启用 DATA6 未出现任何崩溃。

分阶段结果：

| 阶段 | 打开的门 | VoWiFi | VoLTE | 数据 | 隔离设施 |
|---|---|---|---|---|---|
| 基线 | 全关 | 200 OK | registered | 代理出站 OK | — |
| A | `+vowifi_tun_in_namespace` | 200 OK | registered | OK | TUN 进 netns，宿主无泄漏 |
| B | `+three_gpp_ims_sockets_in_worker` | 200 OK | registered | OK | 同上，零 warning |
| C | `+data_proxy_in_worker` | 200 OK | registered | 代理出站 OK | **`wwan1` 迁入 netns** |

阶段 C 的 netns 内容：

```text
lo / wwan1 / save0c9d2870 / sa_vwf0c931974d
flow: 3.16.215.118:5063 <-> 172.20.58.221:7807
flow: 3.16.215.118:5064 <-> 172.20.58.221:7777
宿主 sa_vwf 泄漏 = 0
```

注意阶段 C 之后从**宿主** `ping -I wwan1` 会失败，这是**正确**的：该接口已不在宿主
命名空间；数据仍然通，因为代理的出站 socket 由 worker 在 netns 内创建，实测公网出口
`113.211.112.0`（Maxis），不是 WiFi。

**全隔离下的 worker 崩溃恢复**（§8.1 最后一项，此前一直未验证）：

```text
kill -9 worker → 21142 -> 22160
netns/veth/NAT 各 1，zombies 0
wwan1 仍在 netns 内（接口迁移扛住了重启）
IMS flows 仍 ESTAB 且仅父进程持有 —— fd 泄漏检查 PASS（§4.1.4 的 MSG_CMSG_CLOEXEC）
代理出站恢复后仍为 113.211.112.0
fatal 0，warning 0
```

**结论：UE 隔离架构在真实硬件上、三条业务同时在线的前提下工作正常。** 通话与短信
所需的 VoLTE / VoWiFi 信令均已就绪，待换卡后进行业务测试。`trunk_sockets_in_worker`
仍未验证，因为它需要 Asterisk 配置。

### 8.9 部署路径去平台化：udev 与内核模块（2026-08-23）

本项目最初只面向 410，后来扩到多平台，但部署侧仍停留在"单基带高通"的假设上。这一
轮把它清干净，原则是**运行时观测 > 打包时猜测**。

**删掉的静态 udev 规则**（`deploy/system/99-simadmin-secondary-qmi.rules`）：

- 它匹配 `wwan[0-9]qmi1` / `wwan[0-9]qmi2`，而参考设备实际出现的端口叫 `wwan0at2`
  —— **它从来就没生效过**，只是让人误以为端口已经对 ModemManager 隐藏；
- 端口名是平台相关的：同一条通道在一块基带上叫 `wwan0qmi1`，在另一块上叫
  `wwan0at2`。打包时写死名字，要么完全不匹配（本例），要么更糟 —— 在没见过的硬件上
  把 ModemManager 本该拥有的端口藏起来；
- 它的注释描述的还是已被清除的 `_multi` 模块的行为。

改为 `main.rs::reconcile_secondary_qmi_udev_rules(path, rules)`，只写**实际绑定成功
的那些端口**，落到 `/run/udev/rules.d/`（端口名到基带的映射只在当前 boot 内有效），
然后 `udevadm control --reload-rules` + `udevadm trigger --subsystem-match=wwan`
立即生效。传空规则集时**删除文件** —— DATA6 关闭或全部端点失败时，同一 boot 内早先
写下的规则必须撤掉，否则会把已经该还给 ModemManager 的端口继续藏着（旧代码在
disabled 分支上直接返回，从不清理，是个真实的漏洞）。

发现端点的那条路径本来就已经是平台无关的，无需改动：`discover_primary_qmi_ports()`
读 `/sys/class/wwan/*/type` 并按基带去重，端点只有在 QMI 探测确认 `wds` 之后才被
接受，unit 排在 `Before=ModemManager.service`。

**`deploy/install.sh` 里整段内核模块代码也删了。** 它是这次清理中真正危险的一处：
即便 `secondary-qmi-init` 每次开机都 purge 掉 `rpmsg_wwan_ctrl_multi`，安装器仍会在
安装结束时立刻 `modprobe` 把它加载回来 —— **自己的崩溃修复和自己的安装器对打**。
DATA6 走 in-tree `rpmsg_wwan_ctrl` + `driver_override` 即可，这也是 beta8 在完全没有
树外代码的前提下做到 DATA6 与 IMS 并存的方式。

`purge_legacy_rpmsg_module()` 予以保留。它清理的不是"旧配置"，而是一个会让基带固件
崩溃、代价是整机重刷的内核模块残留；`bind_and_probe()` 里对 legacy 驱动的解绑同样
保留，作为 `rmmod` 被拒（模块被占用或编进内核）时唯一还能拿到通道的兜底。

#### 8.9.1 顺带查出的两个部署期真 bug

清理过程中发现的，都只在**非 410 平台或没有备用通道的硬件**上才会暴露：

1. **`secondary-qmi-init` 在"没有 QMI 控制口"时不发 readiness 就返回 `Ok(())`。**
   unit 是 `Type=notify` + `TimeoutStartSec=75` + `Restart=on-failure`，所以在任何没有
   QMI 控制口的平台上（即多数非高通硬件），systemd 会等满 75 秒判定启动失败，然后
   每 2 秒重启一次，整个 boot 循环下去。这条分支是全函数里唯一漏发 `systemd-notify
   --ready` 的路径。已修：补 readiness，并把 udev 规则一并 reconcile 成空。

2. **unit 的 `ExecCondition` 写死了 `DATA6_CNTL`。** 门打开、但该平台的备用通道不叫
   DATA6 时，条件返回 1、unit 被整体跳过 —— 于是 `purge_legacy_rpmsg_module()` 也一起
   被跳过，而"门开着 + 遗留模块还在"恰恰是最需要 purge 的组合。它同时还重复实现了
   Rust 侧已经做得更好的发现逻辑（`/sys/class/wwan/*/type` + 按基带去重 + QMI `wds`
   探测）。已改为：unit 无条件启动 initializer，由它自己判断；`ExecStartPre` 只保留
   一次平台无关的 `modprobe rpmsg_wwan_ctrl` + `udevadm settle`，全部 best-effort。

unit 里 `SIMADMIN_ENABLE_SECONDARY_QMI=0` 的注释也一并更正 —— 原注释写的是"410 固件
在 AT 端点被强开为 QMI 时会崩"，那是**已被推翻的结论**（真凶是 `_multi` 模块）。默认
仍为 0，但理由改成真实的那个：占用一条通道之前需要在该平台上先验证端点，而不是因为
DATA6 危险。

