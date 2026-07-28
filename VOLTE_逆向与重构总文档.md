# SimAdmin VoLTE 逆向与重构总文档

> 本文整合了此前分散的 6 份 VoLTE 逆向/重构文档，是 SimAdmin VoLTE 的**单一权威参考**。
>
> **覆盖范围**：三个成品二进制（1.1.6-dev18 / 1.1.7-beta / 1.1.7-beta2）的逆向结论、
> IDA Pro 对 beta2 的深度分析、VoLTE 完整链路、QMI 端点真机实测、以及按 beta2 对齐的重构落地状态。
>
> **方法与可信度分级**：
> - **strings 静态推断**：MinGW `strings.exe` 提取 + `event src/*.rs:NNN` 行号锚点重建 + 前端 JS 对照。
>   函数边界/私有名按职责推断，非真实符号；字节级封装以真机为准。
> - **IDA Pro 反汇编**（本次会话，2026-07-27）：对 `simadmin beta2`（aarch64 ELF）做函数级反汇编 +
>   字符串交叉引用，拿到分支结构与常量。这是比纯 strings 更硬的信号。
> - **真机实测**：设备 192.168.100.13（Maxis 50212，MSM8916）与历史高通 410 设备上的运行观察。
>   实测结论**优先级最高**，可推翻静态推断。
>
> 整合自：`VOLTE_深度逆向_含源码对比.md`、`VOLTE_逆向报告_v2.md`、`VOLTE_SMS_逆向分析.md`、
> `VoLTE_1.6_vs_1.7_功能对比与迁移指南.md`、`VOLTE_beta2对齐重构计划.md`、`VOLTE_真机实测结论_QMI端点能力.md`。

---

## 目录

1. [版本矩阵](#1-版本矩阵)
2. [总体架构：借力式 IMS 客户端](#2-总体架构借力式-ims-客户端)
3. [IDA Pro 对 beta2 的逆向结论（本次会话核心产出）](#3-ida-pro-对-beta2-的逆向结论本次会话核心产出)
4. [VoLTE 完整链路](#4-volte-完整链路)
5. [QMI 端点能力矩阵与崩溃机制（真机实测）](#5-qmi-端点能力矩阵与崩溃机制真机实测)
6. [数据承载架构演进：1.6 → 1.7 → beta2](#6-数据承载架构演进16--17--beta2)
7. [语音通话](#7-语音通话)
8. [SIP / IPsec / AKA 技术细节](#8-sip--ipsec--aka-技术细节)
9. [API / 配置 / DB schema](#9-api--配置--db-schema)
10. [错误码 / AT 命令 / 外部依赖](#10-错误码--at-命令--外部依赖)
11. [真机实测记录](#11-真机实测记录)
12. [按 beta2 对齐的重构落地状态](#12-按-beta2-对齐的重构落地状态)
13. [参考资料](#13-参考资料)

---

## 1. 版本矩阵

| 项 | 1.1.6-dev18 | 1.1.7-beta | 1.1.7-beta2 |
|---|---|---|---|
| commit | `05ea96a` | `930365d` | — |
| ELF 大小 | 8,341,216 B | 8,661,944 B | 8,682,760 B |
| 架构 | aarch64-unknown-linux-musl | 同 | 同 |
| 构建时间 | 2026-07-09 | — | — |
| VoLTE 数据通路 | 共享 wwan0（`shared_wwan_data.rs`） | DATA6 secondary-QMI（`managed_mm_data.rs`） | data slot mode（可切换分配） |
| beta2 md5 | — | — | `8745020335e9b7da4af71e3415ea4f56` |

**基线关系**：公开仓库 3899/SimAdmin 1.1.5 是纯 ModemManager 客户端 + axum Web 服务，已含
语音/CS 短信/eSIM/OTA/通知/自动化/DDNS/WLAN 全套，但**无任何 IMS/VoLTE/SIP/IPsec/SIM-Auth 代码**。
VoLTE 二进制在此基线上新增 4 个文件（`volte.rs` ~5,650 行、`ims_sms.rs`、`ims_uim.rs`、
`shared_wwan_data.rs`）+ 对 `modem_manager.rs`（+~672 行）、`handlers.rs`、`main.rs`、
`sms_listener.rs`、`esim.rs`、`config.rs` 的协同改造。

**编译环境痕迹**：编译路径含 `/home/enzhe/.cargo/...`，作者用户名 `enzhe`；静态链接 musl，无外部 .so 依赖。

---

## 2. 总体架构：借力式 IMS 客户端

SimAdmin VoLTE 的核心思路是**"借力"**，自研只写最上层业务逻辑：

- **鉴权借 SIM 卡硬件**：3GPP AKA（AKAv1/AKAv2）全在 USIM 内运算，主机不持 K，通过独立
  SIM Auth 代理走 APDU 透传（错误码族 `sim_auth_*`）。
- **IMS 承载借 ModemManager / QMI**：建 `apn=ims` bearer，主机不自己拨号。
- **信令加密借 Linux 内核 IPsec**：`ip xfrm` 灌 SA/policy，不自研 ESP 加解密。
  （这是与原版 VoWiFi 自研用户态 IKEv2/ESP 的**最大技术差异**。）
- **语音借 ModemManager Voice + AT 直接拨号**（双路径容错）。
- **自研部分**：SIP 信令 + 3GPP SMS 封装/拆包/拼接/去重/RP-ACK + stage 机 + supervisor + runtime 快照。

**没有引入任何 strongSwan / Kamailio / PJSIP / libosmocore 等 IMS/SIP/IPsec 开源栈**——
仅用 `ring` / `rustls` 提供加密原语，SIP/IMS 协议层完全自研。

### 与原版 VoWiFi 的对比

| 维度 | 原版 VoWiFi (`vowifi/`) | VoLTE 版 (`src/volte.rs`) |
|------|------------------------|------------------------------|
| 接入网 | WiFi → ePDG | LTE 蜂窝网 |
| 隧道 | 自研用户态 IKEv2 / ESP | **借用 Linux 内核 IPsec（`ip xfrm`）** |
| IMS bearer | 自己拨 | **让 ModemManager 建 IMS APN bearer** |
| SIP 栈 | `vowifi/ims.rs` | `src/volte.rs`（自研） |
| SMS 封装 | SMS over IPsec | SIP `MESSAGE` + `application/vnd.3gpp.sms` |
| 鉴权 | AKA | AKA（同，靠 SIM 卡硬件） |
| 语音 | — | **新增**：ModemManager Voice + AT 回退 |

---

## 3. IDA Pro 对 beta2 的逆向结论（本次会话核心产出）

> 2026-07-27 用 IDA Pro（9.x + ida-pro-mcp 插件）对 `simadmin beta2` 做函数级反汇编 + 字符串交叉引用。
> base=0x400000，size=0x859828。这是本文档相较旧版最硬的新增信号。

### 3.1 data slot mode（数据槽位分配）—— beta2 的核心新机制

beta2 **不再硬编码"IMS 走主口"**，而是在连接时（`src/volte.rs:1676-1687`）计算一个显式的
**data slot mode**，输入是三个运行时信号，输出两种分配之一。

**决定性字符串**（地址 0x93AAB0 起）：
```
IMS allocated to primary qmi0; DATA6 is reserved for data
IMS allocated to DATA6; primary qmi0 is reserved for data
volte_data_slot_mode_missing
volte_data_slot_conflict
```
**配置意图 token**（三态，声明序）：`independent_wwan1`(0x940867) → `secondary_qmi_data`(0x940878)
→ `both_data_slots_active`(0x94088a)。

**运行时输入字符串**（volte.rs:1676）：`data_requested` / `primary_data_active` / `secondary_data_active`。

**选择函数 `sub_58E0C4`（0x58e0c4，164 字节）** —— 配置字符串 → enum 判别值的解析器，返回 0/1/2：
```
BL sub_467540    ; 读 offset 0x160 处的 "数据是否启用" 标志
CBZ X0, ...      ; 数据未启用 → w20=2
ADRL X2, aIndependentWwa  ; 比对 "independent_wwan1"
  匹配 → w20=0
ADRL X2, unk_940878       ; 比对 "secondary_qmi_data"
  匹配 → w20=1，否则 w20=2
```
- 前置失败 / 数据未请求 → 返回 mode **2**
- 数据请求且能力检查通过 → mode **0**（independent_wwan1）
- 否则再看条件 → mode **1**（secondary_qmi_data）或 **2**

**主流程消费点 `sub_5A2D20`（0x5a2d20，13KB，volte.rs:1676-1687）**：引用全部 data slot 字符串，
是 VoLTE 连接主 async 函数。

> **真机约束（推翻"IMS 可走 DATA6"的一半）**：见 §5。只有"IMS 在主口 qmi0 + qmi-proxy、DATA6 让给
> 数据"这种分配能跑通完整 IMS 流程。`secondary_qmi_data.rs` 模块名（data 不是 ims）印证了这一点。

### 3.2 P-CSCF 四级发现，bearer 先起再读（按 volte.rs 行号）

beta2 的 P-CSCF 发现是**分层降级**的，且**bearer 先建立、再取 P-CSCF**：

| 级 | 来源 | 字符串锚点（volte.rs 行号） |
|---|---|---|
| 1 | **profile 预取** | 1511 `prefetched from IMS profile` / 2162 `volte_runtime_profile_pcscf_missing` |
| — | bearer 起来 | 1590 `Native VoLTE runtime IMS bearer is up` |
| 2 | **直读 QMI WDS** | 2022-2054 `discovered directly from QMI WDS` / `direct QMI WDS P-CSCF query unavailable; keeping AT fallback` / `QMI WDS CID is not numeric; skipping direct P-CSCF query` |
| 3 | **AT CGCONTRDP 兜底** | 2192 `discovered from active IMS bearer` |

> **"WDS 直读"含义**：建数据会话后，网络通过 PCO 把 P-CSCF 随 `--wds-get-current-settings` 一起下发，
> 直接从其输出 `PCSCF address:` 行解析，**不必碰会崩基带的 `$QCPDPIMSCFGE`+`CGACT` AT 序列**。
> 这与旧版本"AT 探测前置、每次连接都跑"正好相反。

### 3.3 QMI provisioning 就绪门

```
/run/qmi_auto_activate.ready
Waiting for initial QMI UIM provisioning to settle
QMI auto-activate ready marker did not appear; continuing with modem readiness checks
```
起流程前等这个 marker 文件（由独立 one-shot 在 SIM 自动激活完成后写），超时则继续（advisory，非硬前置）。

### 3.4 健康检查含 secondary QMI packet-status

```
Secondary QMI packet status was inconclusive; retaining live host IMS state
volte_runtime_health_qmi_disconnected
Secondary QMI packet status query failed; retaining live host IMS state
```

### 3.5 两条 bearer 路径 + 双栈优先→单栈回落

```
--create-bearer=apn=ims,ip-type=,allow-roaming=          ← MM 托管路径
--wds-start-network=apn=ims,3gpp-profile=,ip-type=       ← 原生 QMI 路径
--wds-noop --client-no-release-cid --wds-set-ip-family=  ← CID 分配 + 跨进程复用
Native VoLTE ModemManager dual-stack IMS bearer is ready
... dual-stack IMS activation failed; falling back to single-stack attempts
Native VoLTE secondary QMI IMS WDS bearer started
```

---

## 4. VoLTE 完整链路

### 4.1 stage 阶段机

前端 `volteStatus-*.js` 的 stage 与二进制 `event src/volte.rs:NNN` 完全对应：

```
disabled(未启动)
  → starting(准备启动)
  → identity(读取 USIM) → identity_aka(读取鉴权材料)
  → radio(等待 LTE)
  → modem(等待 ModemManager) → bearer(建立 IMS bearer)
  → pcscf(发现 P-CSCF)
  → register_ipsec / register_udp(IMS 注册)
  → registered(短信已接管)      ← 成功态
  → degraded(等待恢复) / stopping(正在停止)
```
前端折叠成 4 个 UI 步骤：`switch → usim → bearer → register → sms`，按 `phase=degraded` + `last_error`
模式串识别失败步骤。

### 4.2 身份与鉴权（identity / identity_aka）

- 读 IMSI：`AT+CIMI`，失败回落 ModemManager SIM IMSI。
- 解析 USIM AID：`--uim-get-card-status`（筛前缀 `A0000000871002`=USIM / `A0000000871005`=ISIM），
  失败用内置 fallback AID（`Native VoLTE USIM AID discovery failed, using built-in fallback`）。
- 读 SMSC：优先 EF_SMSP（`AT+CRSM=192,28482,0,0,15` + FCP 解析 record_len），回退 `AT+CSCA?`，
  全失败则 SMSC 空串。
- **AKA 在 SIM 卡硬件运算**：nonce 经 APDU 送卡做 AKAv1/AKAv2；AUTS 场景走标准 3GPP 重同步
  （`AKA returned AUTS, requesting resync`）。走独立 SIM Auth 代理（错误码族 `sim_auth_*`）。

### 4.3 SIM Auth 代理（ims_uim.rs 推断）

通道优先级：
1. ModemManager D-Bus `Modem.Sim.SendCommand`（失败 `sim_auth_proxy_connect_failed`）
2. 直接 QMI UIM 客户端 `qmicli --uim-*`（失败 `sim_auth_uim_client_failed`）
3. 逻辑通道 SELECT AID（失败 `sim_auth_logical_channel_failed`）

AUTHENTICATE APDU（ETSI TS 102 220 + 3GPP TS 31.102）：`CLA=00 INS=88 P1=00 P2=<channel> Lc data=<RAND||AUTN>`。
响应：tag 80(RES)/81(CK+IK) 正常，tag 22(AUTS) 重同步。APDU 状态字错误族见 §10。

### 4.4 等 LTE + 建 IMS bearer（radio / modem / bearer）

- 等 ModemManager ready（`Simple.GetStatus` state≥registered，`ModemManager modem is ready for VoLTE IMS bearer`）。
- 建 IMS APN bearer：`--wds-start-network=apn=ims,3gpp-profile=`，环境变量 `SIMADMIN_MM_IMS_BEARER` 可覆盖，
  bearer 路径 `/org/freedesktop/ModemManager1/Bearer/N`。
- 清理旧 bearer：`Deleted stale disconnected IMS bearer`。
- 漫游策略：`recreating IMS bearer to match roaming policy`；禁止时 `volte_runtime_mm_bearer_roaming_forbidden`。

### 4.5 短信接管（registered）★

**MT（收）**：监听 SIP `MESSAGE`（`Content-Type: application/vnd.3gpp.sms`）→ 解 RP-DATA/TPDU →
回 RP-ACK（SIP 202）→ 长短信按 UDH `segment_reference`/`segment_total` 拼接 → 去重 marker 判重 → 入库
（`transport=volte_ims`）→ 触发通知。网络重传只 ACK 不重复入库（`duplicate_count++`）。

**MO（发）**：`/api/sms/send` → GSM7/UCS2 编码 → RP-DATA/TPDU（长短信加 6 字节 UDH，
**UDH 后需 1 个 fill bit 对齐 GSM7 septet**，`95f63ff` 修复点）→ SIP `MESSAGE` 多变体重试
（IPsec 路径 → UDP 路径）。

**与 MM SMS 监听协同**：VoLTE 注册成功则暂停 MM SMS 监听（`SMS listener paused while VoLTE IMS SMS path is registered`）；
未注册时 `/api/sms/send` 自动回退 MM（`VoLTE SMS requested but runtime is not registered; falling back to ModemManager SMS`）；
eSIM 切换时停 VoLTE runtime 并请求 resync。

### 4.6 停止 / 故障恢复

- 配置关闭：runtime stop → 双 SIP socket 关 → xfrm flush → 恢复 MM SMS 监听 → `runtime stopped cleanly`。
- 注册保活：到期前受控重 REGISTER（`IPsec/plain UDP REGISTER refreshed`）。
- 故障自愈：`health_bearer_changed` → supervisor `next_retry_at` 递推 → phase=degraded（UI 显示失败步骤）。

---

## 5. QMI 端点能力矩阵与崩溃机制（真机实测）

> 2026-07-27 实测，设备 192.168.100.13（Maxis 50212，MSM8916，固件 `MPSS.DPM.1.0.c7-00193` [2015-09]），
> libqmi 1.36.0。**此节推翻了"IMS 走 DATA6 独立端点"的旧结论。**

### 5.1 核心结论

**IMS bearer 必须跑在主口 `/dev/wwan0qmi0` + `--device-open-proxy`，不是 DATA6。**

DATA6 上**单发**一条 `--wds-start-network` 确实成功且不崩基带，但 VoLTE 需要
**start-network → get-current-settings → 取 P-CSCF** 的多步流程，而 **DATA6 无法跨进程复用 CID**
（永远 `Transaction timed out`，qmi-proxy 也救不了）。原因：`rpmsg_wwan_ctrl_multi` 暴露的是裸 rpmsg
管道，每次 `open()` 都是新会话。

### 5.2 端点能力矩阵

| 能力 | `/dev/wwan0qmi0`（主口，DATA5_CNTL） | `/dev/wwan0qmi1`（DATA6_CNTL，自编译模块） |
|---|---|---|
| QMI 服务齐全（wds/wda/uim） | ✅ | ✅ `wds 1.36` `wda 1.11` |
| 链路格式 | raw-ip / no QoS header | raw-ip / no QoS header |
| 单发 `--wds-start-network` | ✅ | ✅ 返回 PDH，基带存活 |
| **跨进程复用 CID（关键）** | ✅（需 qmi-proxy） | ❌ `Transaction timed out` |
| `--device-open-proxy` 生效 | ✅ | ❌ |
| ModemManager 占用 | 主口 | MM 标 `(ignored)`，udev 生效 |
| `--wds-bind-data-port` | 未测 | ❌ `InvalidArgument` |
| `--wds-bind-mux-data-port` | 未测 | ❌ `InvalidQmiCommand`（2015 固件不支持） |

**主口 CID 复用实测（决定性证据）**：
```
qmicli -d /dev/wwan0qmi0 --device-open-proxy --wds-noop --client-no-release-cid
→ Client ID not released: Service 'wds' CID '2'
# 复用同 CID
qmicli -d /dev/wwan0qmi0 --device-open-proxy --client-cid=2 --client-no-release-cid --wds-get-packet-service-status
→ Connection status: 'disconnected'  exit=0    ← 真实应答，不是超时
# 同 CID 再查 settings
→ QMI protocol error (15): 'OutOfCall'          ← 合法业务错误（当时无会话），非超时
```
对比 DATA6 上同样操作永远 `Transaction timed out`。

**端口所有权修正**：`/dev/wwan0qmi0` 不是 ModemManager 直接持有——是 **qmi-proxy**（`/usr/libexec/qmi-proxy`，
不在 PATH）持有 fd，MM 通过 proxy socket 通信，自己只 open `wwan0at0/at1`。这正是"主口能跨进程复用 CID"的
机制：第二个 WDS client 走 proxy 与 MM 自己的 bearer 天然共存。

### 5.3 崩溃机制

```
--wds-bind-data-port=N → InvalidArgument（固件不支持）→ 客户端被污染
→ 同 CID --wds-start-network → endpoint hangup → 端口消失 → 基带 SSR
```
性质是**基带 SSR 不是内核 panic**（pstore 空，~375s 自愈），但**有时升级成整机重启**（曾观察到重启后
`uptime` 仅 1 分钟、上次日志停在 `tun: Universal TUN/TAP device driver`）。放大器是失败后的重试循环
（5 次连接 + 3 次重启 MM），已加 `FailureClass::BasebandWedged` 立即中止。

### 5.4 硬约束（写实现必须遵守）

1. **绝不用任何 bind 命令**（`--wds-bind-data-port` / `--wds-bind-mux-data-port`）——直接触发 SSR。
2. **DATA6 上绝不做多步流程**——只能单发。
3. **主口多步流程必须 `--device-open-proxy`** 且 qmi-proxy 在跑。
4. **辅助端点打开必须带 `--device-open-net='net-raw-ip|net-no-qos-header'`**——不带则 CID 分配报 `endpoint hangup`。
5. **Maxis 50212 卡 `ip-type=6` 被网络拒**（`[3gpp] ipv4-only-allowed`）——**先试 IPv4**。
6. **qmicli 一次只允许一个 WDS 动作**（多个报 `too many WDS actions requested`）。

### 5.5 内核模块定位（易误解）

`rpmsg_wwan_ctrl_multi.ko` **不含任何 VoLTE 代码**。内核自带 `rpmsg_wwan_ctrl` 只认 DATA1/DATA4/DATA5。
该模块只给 ID 表加两行让空闲 rpmsg 通道变成设备节点：
```c
{ "DATA6_CNTL", WWAN_PORT_QMI },   // -> /dev/wwan0qmi1
{ "DATA7_CNTL", WWAN_PORT_QMI },   // -> /dev/wwan0qmi2
```
必须内核态的唯一原因：创建字符设备 + 绑定 rpmsg 总线只能在内核做。**修正后角色：给数据 bearer 用，
把主口让给 IMS**（不再是"给 IMS 用"）。

> 注：本仓库的 `SimAdmin/kernel/` 目录已于 2026-07-27 按用户要求删除；该模块的运行时依赖仅在
> "完全照抄 beta2 翻默认"方案（§12）中才需要。

---

## 6. 数据承载架构演进：1.6 → 1.7 → beta2

**这是三版之间最大的改动。SIP/IPsec/AKA 软栈本体三版基本一致，重写的是"IMS bearer 数据面怎么起来"这一层。**

### 6.1 1.6 —— 共享 wwan0（`shared_wwan_data.rs`）

IMS bearer 与默认数据挤在同一个 `wwan0`。字符串：`Shared wwan0 data activated ip=`、
`VoLTE shared wwan0 mode, data disabled:`。qmicli 命令族含 `--wds-bind-data-port`、
`a2-mux-rmnet0/1`（高通 MUX）、路由 `metric 731`。
**问题**：IMS 数据与默认数据抢同一 bam-dmux 通道，激活 IMS PDP 触发固件 DHCP 崩溃。

### 6.2 1.7 —— DATA6 secondary-QMI 独立端点（`managed_mm_data.rs`）

删掉 `shared_wwan_data.rs`，新增两条通路：
1. **managed MM data**：让 MM 管 IMS bearer（`--create-bearer=apn=ims`，读 `bearer.ipv4/ipv6-config.*`），
   双栈失败回落 IPv4。
2. **secondary QMI / DATA6**：新增 CLI 子命令 `secondary-qmi-init`，装成 systemd 服务
   （`Description=SimAdmin DATA6 stock RPMSG QMI initializer`），在 MM 启动前 hold 住 DATA6。
   用 udev `ID_MM_PORT_IGNORE=1` 把 DATA6 从 MM 藏起来，配自编译 `rpmsg_wwan_ctrl_multi.ko`。
   watchdog 能在 MM 恢复后重建 DATA6（`rebuilding DATA6 after the primary QMI restart`）。

systemd unit（1.7 内嵌）：
```
ExecCondition=/bin/sh -c 'modprobe rpmsg_wwan_ctrl ...; 循环等 DATA6_CNTL 出现'
ExecStart=/opt/simadmin/simadmin secondary-qmi-init
```

### 6.3 beta2 —— data slot mode（可切换分配）

见 §3.1。把"IMS/数据各走哪个口"做成显式、可切换、带冲突检测的 data slot mode。
**真机证明能跑通的是"IMS 主口 + DATA6 让给数据"**，与 1.7 逆向文档当初"IMS 走 DATA6"的推测相反（§5 已修正）。

### 6.4 IP family 双栈回落（1.7+ 增强）

| 消息 | 1.6 | 1.7/beta2 |
|---|---|---|
| `volte_runtime_ims_family_unsupported:` | ✗ | ✓ |
| `Dual-stack ... failed; falling back to IPv4` | ✗ | ✓ |
| `IPv6 data configuration failed; retaining IPv4 data` | ✗ | ✓ |

两条路都是 IPv6 优先（ip-type=6）失败回落 IPv4（ip-type=4）；但 Maxis 卡实测须先 IPv4（§5.4）。

### 6.5 非 VoLTE 的 1.7 新增（顺带记录）

Email/SMTP 通知（`EmailConfig`）、ServerChan3 推送、Telegram 自定义 endpoint、
诊断上报（POST 到 `https://simadmin-logs.334455.best/v1/reports`，日志脱敏正则如
`(?i)\b(rand|autn|auts|xres|res|ck|ik|kasme)\b`、`Authorization: [REDACTED]`）、
`prestart_baseband`、`reset-password` CLI。

---

## 7. 语音通话

二进制含完整语音链路（非 IMS Voice/MTSI，走 MM Voice + AT）：

- **拨号**：优先 AT 直接拨号，失败回退 ModemManager Voice D-Bus
  （`AT voice dial failed, falling back to ModemManager Voice`、`CreateCall`、`Voice call started`）。
- **呼叫状态**：`active/alerting/dialing/held/waiting/incoming/outgoing/terminated/busy`。
- **配置**：`CallSettingsResponse`（各种 line presentation/restriction、`hide_caller_id`、
  `voice_call_waiting`；`Only VoiceCallWaiting is supported by ModemManager`）、
  `CallForwardingResponse`（`voice_unconditional/busy/no_reply/not_reachable`、`forwarding_flag_on_sim`）。
- **持久化**：`call_history` 表（direction incoming/outgoing/missed、duration、answered、起止时间）。

---

## 8. SIP / IPsec / AKA 技术细节

### 8.1 REGISTER 头模板

```
Via: SIP/2.0/UDP <ue>:<port>;rport;branch=z9hG4bK...
From: <sip:<imsi>@<ims_domain>>;tag=...
To:   <sip:<imsi>@<ims_domain>>
CSeq: <n> REGISTER
Contact: <sip:...>;+g.3gpp.accesstype="3GPP-E-UTRAN-FDD";+g.3gpp.smsip;expires=3600
Expires: 3600
Supported: path, gruu
Allow: INVITE,ACK,CANCEL,BYE,UPDATE,PRACK,MESSAGE,REFER,NOTIFY,INFO,OPTIONS
Require: sec-agree
Proxy-Require: sec-agree
P-Access-Network-Info: 3GPP-E-UTRAN-FDD
P-Preferred-Service: urn:urn-7:3gpp-service.ims.icsi.sms
Accept-Contact: *;+g.3gpp.smsip
Accept: application/vnd.3gpp.sms
User-Agent: SimAdmin VoLTE
Security-Client: ipsec-3gpp
```

### 8.2 Security-Server / Security-Verify（3GPP sec_agree）

```
ipsec-3gpp;prot=esp;mod=trans;spi-c=<>;spi-s=<>;port-c=<>;port-s=<>;alg=hmac-md5-96;ealg=null
```
初次 REGISTER 带 `Security-Client: ipsec-3gpp`；401 回 `Security-Server`；二次 REGISTER 回显 `Security-Verify`。

### 8.3 Authorization: Digest（AKA）

```
Authorization: Digest username="<imsi>",realm="<>",nonce="<>",uri="sip:<domain>",
  qop=auth-int,nc=00000001,cnonce="<>",response="<md5>",algorithm=AKAv1-MD5
```
AUTS 重同步：401 nonce 超 SQN 范围 → 卡算 AUTS → 带 `auts=` 重发 REGISTER → 新 SQN 401 → 完成。

### 8.4 IPsec xfrm（真机 9.3 节精确语义）

四个独立端口：本地随机 send、本地随机 receive、P-CSCF client（通常 9950）、P-CSCF send（通常 9900）。
```
ip xfrm state add src <ue> dst <pcscf> proto esp spi <spi_s> auth "hmac(md5)" 0x<key> enc "cipher_null" "" mode transport
ip xfrm policy add ... dir out tmpl ... proto esp mode transport
```
- 出站 policy：`UE local_send → P-CSCF port-s`，SA 用 Security-Server `spi-s`。
- 入站 policy：`P-CSCF port-c → UE local_receive`，proto udp 锁端口。
- **`cipher_null` 加密密钥必须空字符串**（写 `0x` 会被内核拒 `RTNETLINK answers: Invalid argument`）。
- SA 只限 IPv6 源/目的与 ESP/SPI，不在 `sel` 锁 UDP 端口；UDP 端口只在 policy selector。
- 清理：`ip xfrm policy flush` / `state flush` / `ip -6 route del`。IPv6 地址加 `nodad noprefixroute`。
- **双模容错**：IPsec 注册失败降级明文 UDP SIP（`IPsec registration failed, falling back to plain UDP SIP`），
  两条路径的 MO/MT/RP-ACK 监听代码同构。

---

## 9. API / 配置 / DB schema

### 9.1 VoLTE 专属 API（相较公开源码的真正增量）

```
GET  /api/volte/control            # runtime 快照
POST /api/volte/feature            # 开关（VolteConfig{feature_enabled, sms_enabled}）
POST /api/volte/diagnostics/upload # beta2 新增
GET  /api/ims/status               # 占位错误（未改动）
GET  /api/voicemail/status         # 占位错误
```

### 9.2 runtime 快照字段（`/api/volte/control`）

```
phase, stage, registration_mode, pcscf,
session_started_at, registered_at, last_rx_at, last_tx_at,
last_error, last_failure_at, next_retry_at,
sent_count, received_count, duplicate_count, reconnect_count,
data_path_mode, data_path_probe
```
前端事件 `simadmin-volte-control-updated`；从 `runtime.registered` / `phase==="degraded"` / `stage` 派生 UI。

### 9.3 完整 API 分组（其余为公开源码既有）

认证/系统、设备/SIM/网络/蜂窝（radio-mode、band-lock、cell-lock、operators、data、roaming、
airplane-mode、baseband/restart、work-mode）、通话/IMS/语音信箱、SMS、eSIM、通知、自动化、
OTA/DDNS/WLAN/device-network。详见原 `VOLTE_逆向报告_v2.md` §7（已并入本文，如需逐条恢复见 git 历史）。

### 9.4 配置结构体（serde）

```
struct AppConfig { apn, auth_method, webhook, notifications, device_network,
    version_update_notifications, roaming_allowed, data_enabled, work_mode,
    automation, volte, security, esim, ... }
struct VolteConfig { feature_enabled, sms_enabled }   # beta2 另有 voice_enabled/ip_family_preference 等（见当前源码）
```
通知/eSIM/DDNS/自动化/OTA 等结构体清单见 git 历史 `VOLTE_逆向报告_v2.md` §10。

### 9.5 数据库 schema（SQLite `data.db`）

- `sms_messages`：`id, phone_number, sms_center, source, timestamp, updated_at, direction, transport`
  （`volte_ims`/`cs`/`vowifi`），`notification_status`。默认上限 10,000 条（可配 100-100,000），按日清理最旧。
- `call_history`：`id, direction, phone_number, duration, start_time, end_time, answered`。
- `auth_config`（含 `admin_password_hash`）、`auth_sessions`、`automation_log`、`notification_log`、`notification_queue`。

---

## 10. 错误码 / AT 命令 / 外部依赖

### 10.1 错误码族（`volte_*`）

- **依赖/命令**：`volte_dependency_missing:ip`、`volte_command_failed:mmcli`、`volte_command_spawn_failed/timeout/wait_failed`、`volte_at_read/write/timeout_failed`
- **身份/鉴权**：`volte_imsi_missing`、`volte_usim_aid_missing/not_usim`、`volte_usim_aka_failed`、`volte_aka_material_invalid/res_empty`、`volte_register_nonce_not_aka`、`volte_digest_*`
- **SIM Auth 代理**：`sim_auth_proxy_connect_failed/open_failed`、`sim_auth_uim_client_failed`、`sim_auth_logical_channel_failed/close_failed`、`sim_auth_apdu_exchange_failed/security_status/build_failed`、`sim_auth_apdu_more_data_unhandled/wrong_length(_unhandled)/instruction_not_supported/class_not_supported`、`sim_auth_aka_response_parse_failed/empty/success_parse_failed/sync_failure_parse_failed/unknown_tag`、`sim_auth_retry_not_attempted`
- **IPsec/端口**：`volte_ipsec_ik_invalid`、`volte_ipsec_requires_ipv6`、`volte_ipsec_udp_bind_failed/recv_udp_bind_failed`、`volte_security_server_missing`、`volte_random_port_range_invalid/port_invalid/spi_invalid`
- **注册**：`volte_register_initial/auth_unexpected_status`、`volte_register_auth_send_failed`、`volte_ipsec_register_*`、`volte_ipsec_auts_*`、`volte_runtime_all_pcscf_failed`、`volte_runtime_profile_pcscf_missing`
- **bearer/漫游**：`volte_runtime_mm_bearer_connect_failed/path_missing/not_connected/roaming_forbidden`、`volte_runtime_mm_family_unsupported`、`volte_runtime_mm_modem_present/wait_timeout`、`volte_runtime_health_bearer_changed/query_failed`、`PdpAuthFailure`
- **data slot（beta2）**：`volte_data_slot_mode_missing`、`volte_data_slot_conflict`、`volte_data6_start_failed/activation_failed`、`volte_secondary_qmi_wds_cid_missing`、`secondary_qmi_data_registration_not_home/missing`、`secondary_qmi_device_unavailable`
- **SMS**：`volte_sms_encode_failed`、`volte_smsc_missing`、`volte_phone_uri_invalid`、`volte_sms_message_all_variants_failed`、`volte_ipsec_sms_all_variants_failed`、`volte_runtime_(ipsec_)mt_rp_ack_send_failed`
- **P-CSCF/PCO**：`volte_cgcontrdp_ipv6/ipv4/gateway_missing`、`volte_pcscf_family_mismatch`、`volte_ip_settings_missing`

### 10.2 AT 命令清单

| 命令 | 用途 |
|------|------|
| `AT+CIMI` | 读 IMSI |
| `AT+CNUM` | 读本机号码（多厂商隧道变体） |
| `AT+CSCA?` | 读 SMSC |
| `AT+CRSM=192,28482,0,0,15` | 读 EF_SMSP（含 FCP 解析 record_len） |
| `AT+CGDCONT=<cid>,"IPV6","ims"` / `,"IPV4V6",""` | IMS APN context 配置/恢复 |
| `AT+CGACT=<0/1>,<cid>` | 激活/去激活 PDP context |
| `AT+CGCONTRDP=<cid>` | 读 PDP context + PCO（含 P-CSCF） |
| `AT$QCPDPIMSCFGE=<cid>,1,1,1` / `=<cid>,0,0,0` | **Qualcomm 专属**：开/关 PCO 中 P-CSCF 主备返回 |
| `AT+CEER` / `AT+CLCC` / `AT+CPMS` | 错误原因 / 通话列表 / SMS 存储 |

**Qualcomm PCO 必要序列**（默认 CID 2，会崩基带的风险序列）：
```
AT+CGACT=0,<cid> → AT+CGDCONT=<cid>,"IPV6","ims" → AT$QCPDPIMSCFGE=<cid>,1,1,1
→ AT+CGACT=1,<cid> → AT+CGCONTRDP=<cid> → AT+CGACT=0,<cid>
→ AT$QCPDPIMSCFGE=<cid>,0,0,0 → AT+CGDCONT=<cid>,"IPV4V6",""
```
`AT$QCPDPIMSCFGE=<cid>,1,1,1` 是让基带在 PCO 返回 P-CSCF 主备的关键开关。
> **beta2 已把此序列降级为最后兜底**（§3.2）：优先 WDS 直读，拿不到才跑 AT。

### 10.3 外部依赖命令

`ip`（内核 IPsec/路由，缺失 `volte_dependency_missing:ip`，OpenWrt 需 `ip-full`）、`mmcli`、
`qmicli`（`--uim-get-card-status`、`--wds-start-network/stop-network/get-current-settings`、
`--wds-set-ip-family`、`--wds-noop`、`--device-open-proxy`、`--client-cid`、`--client-no-release-cid`；
`qmi-proxy` 在 `/usr/libexec`，不在 PATH）、ModemManager D-Bus、NetworkManager、`systemctl`、
`tar`/`unzip`/`chmod`（OTA）、`lpac`（eSIM）、`iptables`。

---

## 11. 真机实测记录

### 11.1 设备清单

- **192.168.100.13**（当前主力，Maxis 50212，MSM8916，Debian 13 trixie，内核 6.17.0-rc6）：
  密码 `1313144`，hostkey `SHA256:9/NFdvi+PH2k3/WI9nPDTLPX8bAR7/X3ULxwvGt/HOA`。
  登录：`plink.exe -ssh -batch -pw 1313144 -hostkey <key> -m <脚本文件> root@192.168.100.13`
  （用 `-m 脚本文件`，不要内联转义）。
- **10.0.0.116**（历史电信 46011 设备，MSM8916 固件 2015）。

### 11.2 高通 410 参考成品端到端成功（历史）

同机同卡下参考成品 `05ea96a` 真实完成完整闭环：
```
P-CSCF discovery → IMS IPv6 bearer → REGISTER 401 → USIM AKA（含 AUTS）
→ Linux XFRM → protected REGISTER 200 → IPsec listener
→ MT multipart SMS → RP-ACK(SIP 202) → 两段拼接入库
```
修正了"运营商/基带没提供 P-CSCF"的旧判断——真实原因是缺 `AT$QCPDPIMSCFGE` PCO 启用命令。

### 11.3 clean-room 重构版历史验证（高通 410）

- `cfc34b1`：双栈 bearer + `ipv6_first`/`ipv4_first`/单栈策略、Qualcomm PCO 主备发现、
  401/AUTS/USIM AKA、2 SA + 入/出各 1 policy、受保护 REGISTER 200，
  状态 `registered`/`ipsec`/`dedicated_ims_bearer_ipv6`。
- `a09ec36`：保存并使用运营商下发的 `P-Associated-URI`（修复前 `403/Invalid User`，修复后 MO 202）。
- `95f63ff`：长短信 UDH 后 GSM7 septet fill-bit 对齐（239 字符两分片用户确认无乱码）。
- `05d7d52`：CS/VoLTE/VoWiFi 统一显示；476 Rust 测试全绿、Clippy 零告警。

### 11.4 10.0.0.116 电信卡的阻塞（历史）

MSM8916 固件 2015，电信 46011，已 registered LTE。**任何激活 IMS PDP context 的动作都触发
`dhcp_client_mgr.c:263` 基带崩溃**：`mmcli --create-bearer=apn=ims,ip-type=ipv4v6`→`Ipv6OnlyAllowed`，
`ipv6`→`prefix-unavailable`，`qmicli --wds-start-network=apn=ims,ip-type=6`→崩基带，
`$QCPDPIMSCFGE`+`CGACT=1,2`→崩基带。SIM 有 `fixed-dialing` 锁 + `sim-pin2`（用户无 PIN2）。
**未确认假设**（需换卡实测）：号码未在电信侧开通 VoLTE（最可能）/ FDN 拦 IMS APN / 固件 DHCP bug。
对照"能开 VoLTE 的那台"与本设备非同一设备、非同一张卡。

### 11.5 192.168.100.13 主口 CID 复用逐步验证（2026-07-27）

```
STEP1 主口 wds-noop 分配 CID          → CID '3'                     ✅
STEP2 跨进程复用同 CID(packet-status)  → disconnected（真实应答）     ✅
STEP3 同 CID set-ip-family=4           → exit 0                      ✅
STEP4 同 CID current-settings          → OutOfCall（健康空会话应答）  ✅
STEP5 释放 CID                         → exit 0                      ✅
辅口 wwan0qmi1 数据格式                → raw-ip / QoS header no       ✅
```
**唯一未验证的一步**：`--wds-start-network=apn=ims,ip-type=4` 主口激活是否成功且不崩基带。

---

## 12. 按 beta2 对齐的重构落地状态

> 详细计划与决策见本目录 `VOLTE_beta2对齐重构计划.md`（保留，作为工作计划文档）。
> 本节是落地状态摘要。现有代码本身已是 beta2 的 clean-room 克隆，架构层 ~90% 对齐，
> 差距只在几处**行为顺序**与**缺失的分层**。

### 12.1 已落地（代码 + 152 测试全绿，纯代码验证，未上真机）

- **阶段 1 — P-CSCF 发现顺序（`live.rs` + `pcscf.rs`）**：把会崩基带的 AT `$QCPDPIMSCFGE`+`CGACT`
  探测从"每次连接前无条件跑"改成 beta2 顺序——**bearer 先起 → 用 bearer 自带 P-CSCF（MM 的 PCO /
  native 的 WDS 直读）→ 只有拿不到才最后跑 AT 兜底**。移除了不再需要的 prefix-unavailable workaround。
- **阶段 2 — data slot mode（新增 `data_slot.rs`）**：三态 enum（`PrimaryImsSecondaryData` /
  `SecondaryImsPrimaryData` / `PrimaryImsOnly`）+ 冲突检测 + `select_data_slot_mode()`，纯逻辑可测；
  `data_path_mode` 从展示串升级为由 enum 派生。token 对齐 beta2（`independent_wwan1` /
  `secondary_qmi_data` / `both_data_slots_active`）。
- **阶段 4 — QMI 就绪门（新增 `readiness.rs`）**：接入 `/run/qmi_auto_activate.ready` 等待（超时则继续）。

### 12.2 按决策保持现状（有意为之）

- **阶段 3 — native 主口路径转正为默认**：**保持 env 门控 `SIMADMIN_VOLTE_NATIVE_IMS_BEARER`，
  默认仍走 ModemManager**。原因：`--wds-start-network=apn=ims` 主口激活这步在参考基带上从未真机验证，
  坏激活可能 SSR。接线逻辑就位，但入口仍门控。
- **阶段 4 后半 — secondary QMI packet-status 健康检查**：记入文档备用，未加代码（只对默认关闭的
  native 路径有意义）。现有健康机制是 REGISTER 刷新周期（失败转 Degraded 重建）。

### 12.3 "完全照抄 beta2 翻默认"兜底清单（后期若默认 MM 仍无法激活/仍崩基带）

触发条件：默认 MM 路径在真机仍 `Ipv6OnlyAllowed`→崩基带，或拿不到 P-CSCF。步骤：
1. IMS bearer 跑主口 `/dev/wwan0qmi0` + `--device-open-proxy`；DATA6/DATA7 只给数据。
2. `--wds-noop --client-no-release-cid` 分配 CID → 复用同 CID → `--wds-set-ip-family`
   → `--wds-start-network=apn=ims,ip-type=4`（先 IPv4）→ `--wds-get-current-settings` 取 P-CSCF。
3. 遵守 §5.4 全部硬约束（绝不 bind、辅口带 net flags、主口带 proxy、一次一个 WDS 动作）。
4. netdev 解析：起会话后逐个 `wwanN` 发探测流量按 rx 计数判定，探不到回落 `Assumed`。
5. wedge 识别（`endpoint hangup`/`interface-in-use-config-match`/`MobileEquipment.Unknown`）→
   `FailureClass::BasebandWedged` → 立即中止整个重试批次，**不回落 MM**。
6. 翻默认：`native_ims_bearer_enabled()` 默认改 true，env 变"强制关闭"开关。
   **翻默认前必须真机验证 §11.5 的最后一步（主口 IMS 激活不崩基带）**。
7. DATA6 运行时依赖：`secondary-qmi-init` 在 MM 启动前 hold 住 DATA6；udev 给所有空闲 QMI 口写
   `ID_MM_PORT_IGNORE=1`（含 DATA7 缺口）；watchdog 在主 QMI 重启后重建 DATA6。

### 12.4 环境/工具注意事项

- **Edit 工具往文件注入 BOM 的坑**：本次会话踩过——`mod.rs` 中间被插入 UTF-8 BOM（`ef bb bf`）导致
  rustc 无法解析 `pub mod` 声明。改文件后应用 PowerShell 客观核实（findstr / hexdump），
  Read/Grep 工具对本仓库偶发失真返回，不可轻信。
- **交叉编译**：`cargo zigbuild --release --target aarch64-unknown-linux-musl`（普通 `cargo build --target`
  会因 `ring` 需 aarch64 C 编译器失败）。zig + cargo-zigbuild 已装。
- **设备编译内核模块**：Debian 13 trixie，`CONFIG_MODULE_SIG_FORCE` 未开，自编译 `.ko` 可直接加载。

---

## 13. 参考资料

- **规范**：3GPP TS 24.229（IMS SIP）、TS 24.341（SMS over IP）、TS 24.011（RP/CP）、
  TS 23.040（SMS TPDU）、TS 24.301（EPS bearer）、TS 33.203（IMS 接入安全）、TS 33.102（AKA 重同步）、
  RFC 3261（SIP）、RFC 3310（Digest AKAv1）、RFC 4169（AKAv2）、RFC 3329（sec_agree）、
  RFC 2617/2104（Digest/HMAC）。
- **对照实现**：Open5GS、Kamailio（P-CSCF/IMS 侧）；Android IMS 栈 / ims HAL；本仓库原版 `vowifi/`。
- **当前源码 VoLTE 模块**：`SimAdmin/backend/src/access/volte/`（`live.rs`、`bearer.rs`、`pcscf.rs`、
  `native_bearer.rs`、`data_slot.rs`、`readiness.rs`、`ipsec.rs`、`sip.rs`、`sms.rs`、`plan.rs`、`runtime.rs`），
  QMI 层 `SimAdmin/backend/src/cellular/`（`qmi_wds.rs`、`qmi_netdev.rs`、`secondary_qmi.rs`）。
- **工作计划**：`SimAdmin/VOLTE_beta2对齐重构计划.md`。
