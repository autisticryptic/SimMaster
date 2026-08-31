# SimAdmin 架构说明

> 这份文档解释 SimAdmin 怎么组织多线路、IMS 接入和网络隔离，供读代码前先建立整体印象。
>
> 只写当前生效的设计和它背后的原因。已完成的改造过程、验收记录和历史决策都在 git 历史里，不在这里重复。待办事项见 `DEVELOPMENT_PLAN.md`。

## 1. 核心概念：线路（line）

SimAdmin 管理的最小单位不是"设备"也不是"SIM 卡"，而是**线路**。一条线路是一个可以独立注册 IMS、打电话、收短信、跑流量的实体。

线路有两类来源：

- **基带线路** —— 一个物理卡槽加一个 UIM slot
- **读卡器线路** —— 一个独立的 SIM/eSIM 读卡器

线路 ID 的生成方式是这个设计的关键（`hardware/cellular/modem_manager.rs`）：

```rust
physical_line_id  = md5("physical-line" \0 hardware_key[#uimN])
```

`hardware_key` 是物理槽锚点；`uim_slot` 只在 > 1 时以 `#uimN` 追加，所以单 UIM 设备的 ID 不会因为加了这个维度而变化。

**注意 material 里没有 ICCID。** 同一个槽换一张卡，`line_id` 不变。这样"这个槽位的配置"（数据连接开关、代理、VoLTE profile 顺序、Trunk 端点）跟着物理位置走，换卡不丢。

旧版是 `md5(line_key \0 sim_key)`——**含 SIM**，所以换卡就变成另一条线路。这些旧 ID 由 `physical_line_identity()` 算出来放进 `legacy_line_ids`，首次发现时迁移线路配置、自动化目标、通知作用域和累计流量（流量是事务合并后删除旧行，重复刷新不会二次累加）。

反过来，跟 SIM 卡本身绑定的东西用另一个键：

```text
SimBindingKey = ICCID          （普通卡）
              = EID + profile ICCID   （eSIM）
```

IMS 覆写（自定义 IMSI、ePDG DNS、P-CSCF 覆盖）用这个键，所以卡换到别的槽位，这些设置跟着卡走。

**两套键各管一摊，是刻意的。** 混用会导致换卡后要么丢配置，要么把上一张卡的 IMS 身份套到新卡上。

读卡器线路同理：`reader_line_id(reader_id, uim_slot)` 以读卡器本身为锚点，插入另一张卡不会顶掉用户已有的 VoWiFi/trunk/通知/自动化配置。

短信、通话和诊断历史保留写入当时的原始 ID，作为历史审计事实，不做批量改写。

## 2. 前端信息架构

SIM 页面是两栏工作台：

- **左栏** —— 设备列表，基带线路和读卡器线路混在一起，读卡器是一级公民
- **右栏** —— 只显示当前选中线路，六个标签：概览 / eSIM / IMS 与 Trunk / 短信 / 通知 / 自动化

读卡器不依赖基带，所以有概览、eSIM、VoWiFi、短信、通知、自动化；基带专属的 VoLTE、蜂窝数据、Trunk 控制不显示。

### 接入状态只显示一个

线路摘要区同一时间只显示一种语音接入，按优先级选：

1. VoWiFi 已注册
2. VoLTE 已注册
3. VoWiFi 已启用且正在连接
4. VoLTE 已启用且正在连接
5. 都没启用 → CS

进度条按接入类型分段：VoWiFi 和 VoLTE 各 6 阶段，CS 4 阶段。CS 的第 4 阶段目前用 ModemManager 的连接状态近似，后端补上真正的 CS voice capability 后应该换成那个字段——现在不能宣称 CS 音频链路已完成。

### 离线线路仍可配置

线路离线后配置仍能保存。离线保存只写持久化意图，不去调用已经不存在的 ModemManager 对象；设备回来时由线路恢复器应用。

离线基带的"重启此基带"也保留：优先用该线路保留的 QMI 控制口发 reset，并且只接受映射回同一个 QMI 设备的 ModemManager 对象。QMI reset 之后还没重新枚举，才回退到重启 ModemManager——那一步会短暂影响其他基带，所以有确认框。

## 3. 网络路由隔离

这是整个项目最容易出错的地方，值得单独理解。

### 问题

运营商在 SIP/SDP 里给的 P-CSCF、RTP、RTCP、视频地址通常不是同一个地址。如果只把 P-CSCF 写进主路由表：

- 动态媒体地址会落到管理网口（`wlan0`），媒体逃逸
- 多卡同时跑时，后建立的线路的 `/32` 路由会覆盖先建立的

### 路由域

`backend/src/platform/network_routing.rs` 统一分配表号：

| 域 | 表号基址 | 规则优先级基址 | 用途 |
|---|---|---|---|
| `ModemData` | `12000` | `10000` | 数据代理、流量任务 |
| `VolteIms` | `14000` | `14000` | VoLTE P-CSCF、RTP、RTCP、视频 |
| `VowifiIms` | `16000` | `18000` | VoWiFi ePDG TUN、P-CSCF、RTP、视频 |

同域内的实际表号是 `table_base + 接口槽位 * 2 + (是否 IPv6)`：`wwanN` 用 `N` 作槽位，TUN/USB/MBIM 用接口名哈希。乘 2 再加地址族位，是为了让同一接口的 v4/v6 各占一个表号而不互相覆盖。

**槽位必须稳定**——线路重启后表号不能变，否则旧规则会残留指向一个已经被别人用掉的表。

每个承载建一条源地址规则：

```text
ip rule add priority <line-priority> from <ims-address>/32 table <line-table>
ip route replace <remote-media>/32 dev <line-interface> table <line-table>
```

这样同一个远端 RTP 地址可以同时出现在多条线路里，内核按源地址选表，而不是按主表最后一条路由。

### 源地址规则不够，还要绑接口

如果两个接口拿到**完全相同**的地址（`wwan0: 10.0.0.2` 和 `wwan1: 10.0.0.2`），`ip rule from 10.0.0.2/32` 分不出接口。所以所有主动建连的 socket 还必须绑接口：

- 数据代理每个出站 TCP 用 `SO_BINDTODEVICE`
- VoLTE SIP、P-CSCF DNS、RTP/RTCP/视频 relay 绑对应 `wwan*`
- VoWiFi SIP/RTP 绑该线路独有的 `sa_vwf...` TUN
- QMI 承载识别探测 socket 绑当前候选 `wwan*`，否则会把另一条线路的 DNS 响应当成自己的

**没有显式 socket 的第三方进程或内核透明转发流量，靠 `from` 规则区分不了。** 那种需求要上 `fwmark`/VRF/netns，并且所有入口统一继承线路标记。当前项目内的代理、流量、IMS 信令和媒体都有显式 socket，所以够用。

### 新设备接入约束

设备适配层只需要提供承载接口名、本地地址前缀、建立/释放钩子。

**不允许**在设备或 IMS 实现里直接往主路由表写动态 P-CSCF/RTP 路由，也不允许用固定接口名（`wwan0`）代替线路绑定。新增设备复用 `RouteDomain`，并为多线路、相同远端地址、重连、释放加隔离测试。

## 4. VoLTE profile 选择

profile 以 **SIM 的归属 PLMN** 为准，绝不把 `modem.3gpp.operator-code` 在漫游时当归属运营商。归属 PLMN 按顺序取：

1. SIM 对象属性
2. 与 IMSI 前缀一致的已注册运营商
3. USIM EF_AD 的 MNC 长度
4. 最后才让 catalog 按 IMSI 推断

自动匹配找不到可用 LTE profile 时，生成标准 3GPP 派生 profile（`ims.mncXXX.mccYYY.3gppnetwork.org` + `ims` APN + 通用 REGISTER 策略），并在运行状态里用 `profile_source=derived` 和 `profile_fallback_reason` 明确标记。

**显式 pinned profile 严格失败，不会被默默替换成派生的。**

当前注册 PLMN 与归属 PLMN 不同、且 profile 允许接入网络头时，REGISTER 动态加 `P-Visited-Network-ID`。这个头只影响漫游上下文——IMS 域名、APN、AKA、registrar 和安全策略仍来自归属 profile。

每条线路还有三个有序的 profile 候选槽位（用户数据库 → 下载 catalog → 派生兜底），顺序按线路独立保存，不是全局设置。来源不可用时该槽位改用派生 profile，但不去重——三个槽位可以都解析成同一个派生 profile 并实际执行三次。

## 5. 配置存储

继续用版本化 JSON，没有为自动化/通知拆关系表：

- 自动化用 `task.target.line_id`
- 通知用 `rule.sim_channel_ids`（基带是 `line_id`，读卡器是 `reader:<slot_id>`）
- lpac reader 参数在 `LineProfileConfig.esim_reader`，按线路存

配置规模小的时候版本化 JSON 比拆表更容易保持向后兼容。满足下面任一条件再考虑迁移：

- 单设备几百条任务/规则
- 需要多客户端并发局部更新
- 需要按线路做 SQL 聚合、外键、事务更新

迁移时必须给线路字段建索引，并保留旧 JSON 的一次性导入。

## 6. 相关文档

| 文档 | 内容 |
|---|---|
| `DEVELOPMENT_PLAN.md` | 待办与验收计划（唯一的 TODO 来源） |
| `IMS_REGISTER_TRISTATE_SCHEMA.md` | REGISTER 三态字段（`true`/`false`/`omit`）契约 |
| `QCM410_BAM_DMUX_MODEM_CRASH.md` | 410 基带崩溃分析与恢复 |
| `ue-isolation-migration.md` | UE 隔离（netns/veth）迁移设计 |
| `CARRIER_PROFILES.md` | carrier catalog 来源与限制 |
| `INSTALL.md` / `ENVIRONMENT.md` / `DEVELOPER.md` | 安装、运行环境、开发构建 |
