# MM IMS Exact-Family Profile Lease Design

> 状态：设计稿，未接入生产代码，未在设备执行。
> 目标：解决 ModemManager 1.18 中有效 `profile-id` 优先于请求 `ip-type` 的约束，同时保持单一 MM owner、UE namespace 隔离和可回滚恢复。

## 背景

beta8 的静态证据显示，IMS 承载准备同时关注 PDP/CID、地址族、APN 和 P-CSCF reporting。当前 SimAdmin 已能把请求地址族完整传给 MM，并读取实际授予的 IPv4/IPv6 配置；但当 MM profile pin 的内置族与网络 forced-family 结果冲突时，只修改请求标签不会改变 profile 本身。

已观察到的失败形态：

- 请求 `ipv4v6`、pin `profile 2`，MM 实际只授予 IPv6；
- 临时 IPv4 profile 被 `pdn-ipv4-call-disallowed` 拒绝；
- 保留该 IPv4 pin 后再请求 IPv6，仍会重复使用 IPv4 profile，不能形成有效 IPv6 重试。

本设计只处理 profile-family 语义，不声称能制造运营商 PCO/P-CSCF，也不改变 SIP/AKA/IPsec 策略。

## 不变量

1. 同一物理 modem 只有 MM 一个 bearer owner；不借用 MM 内部 WDS client，不同时启动 direct-QMI owner。
2. 原有 profile、Initial EPS 设置、其他线路 context 和普通数据 profile 不被覆盖。
3. 临时 profile 的所有属性、创建结果、MM unique owner、进程、bearer 和 generation 都进入归属账本。
4. 任何取消、超时、owner 变化、结果不确定或清理失败都保留账本，不猜 ID、不盲删、不自动重试写操作。
5. P-CSCF 只能来自同一个 retained MM bearer、经过归属校验的活动 context、已验证配置或 UE-bound DNS；IP 地址本身不等于 IMS 注册。
6. 临时 profile 成功创建不等于 bearer 成功，更不等于 SIP/AKA 注册成功。

## Lease 状态

```text
Absent
  -> SnapshotVerified
  -> Creating
  -> CreatedOwned
  -> BearerAttempted
  -> Released

任何状态的 owner/generation/列表不确定或清理失败
  -> ReconciliationRequired
```

`SnapshotVerified` 必须包含：

- MM bus unique owner 与 modem object path；
- 完整 ProfileManager 列表的规范化快照；
- 目标 profile 是否存在；
- 完整 profile 字段，不只保存 `profile-id`、APN、PDP type；
- InitialEpsBearerSettings 快照；
- 当前活动 bearer/context 和接口归属；
- 当前线路 generation、进程身份和管理路由状态。

## 创建策略

### 仅在有明确 family 冲突时创建

普通 IMS 首次连接继续使用已验证的显式 profile pin。收到结构化 `NetworkForcedIpv4`/`NetworkForcedIpv6` 后：

1. 如果没有 profile pin，可以按现有计划执行结构化 single-family retry。
2. 如果存在 profile pin，先比较 pin 对应 profile 的实际 family。
3. family 已匹配时不创建临时 profile；重新尝试必须有明确的网络/生命周期理由。
4. family 不匹配时停止同 pin retry，生成 `profile_pin_family_conflict`。
5. 只有在独立维护策略允许、目标 profile 能力已确认、且没有活动冲突时，才由 ProfileManager 严格新建临时 profile。

### ProfileManager 约束

- `Set` 省略 `profile-id` 才表示严格新建；不向 `Set` 提交现有 ID，避免把现有 profile 当作可修改对象。
- 新 profile 至少记录 APN、ip-type、认证/漫游字段及 MM 返回的完整属性。
- 返回 ID 必须在同一 MM owner 下重新 `List` 验证，并与返回属性逐字段一致后才登记归属。
- 列表顺序、序列化格式变化不能被误判为 profile 内容变化；字段值变化必须阻断自动清理。
- 不自动修改 InitialEpsBearerSettings。若目标是初始 EPS 重新协商，必须是单独的维护流程。

## Bearer 尝试

临时 profile 的 bearer 请求必须携带返回的 pin，并记录：

- requested family；
- MM `Properties.ip-type`；
- actual `Ip4Config` / `Ip6Config`；
- bearer path、unique owner、interface；
- P-CSCF/DNS/PCO 来源；
- worker generation 和 namespace receipt。

只有 actual family 与目标结果一致时才允许继续配置 UE 网络。单族实际授予可以作为明确的降级结果记录，但不能把请求双栈写成实际双栈。

## 清理与恢复

正常成功或失败清理顺序：

1. 停止接受新的同线路 bearer 操作；
2. 确认 retained bearer 仍由原 MM owner 持有；
3. 释放/断开本次 bearer；
4. 确认 interface、地址、路由和 namespace receipt 已归还；
5. 删除唯一由本 lease 创建的临时 profile；
6. 重新 `List` 并逐字段比较原 profile 快照；
7. 只有全部验证成功才删除 lease 账本。

如果 profile 原本不存在，恢复动作必须是 MM 明确支持的删除操作，而不是写入空 APN 占位 profile。不能无条件把 reporting 写成 `0,0,0`；必须保存原始值并恢复原始状态。任何一步失败都进入 `ReconciliationRequired`。

进程崩溃、MM 重启或 owner 变化后：

- 先核对原 bus ID/unique owner/generation；
- 不把复用的 modem/profile/bearer 编号当作原资源；
- 只对归属明确且 owner 仍匹配的资源执行清理；
- 不确定时保留账本并要求维护窗口处理。

## 与 beta8 的关系

可借鉴：

- profile/CID/family/APN 是一个注册前计划；
- P-CSCF reporting 要在正确 profile 时序中准备；
- MM 和自有 WDS 是不同 owner 路径；
- P-CSCF 缺失时进行有界多轮观察。

不直接移植：

- beta8 自有 WDS client；
- 未证实的临时 `CGACT=1` prefetch 流程；
- 宽泛 IPsec 错误到 plain UDP 回退；
- 全局或未确认 namespace 的 XFRM flush；
- 固定 profile/CID、默认三位 MNC 猜测或硬编码 P-CSCF。

## 离线测试门槛

在任何设备窗口前必须有：

- ProfileManager `Set` 严格新建、返回 ID、列表复核的 fake D-Bus 测试；
- 字段完整快照和规范化比较测试；
- profile pin/family 冲突不重复请求的测试；
- 创建中取消、Connect 超时、owner 替换、列表变化、删除失败和未知结果测试；
- 原 profile 不存在时删除而非空 profile 恢复的测试；
- 双架构 Actions 编译和回归；
- 文档明确区分代码、CI、设备 bearer、P-CSCF、SIP/AKA 和自然续期验收。

## 设备窗口门槛

未来首次验证只允许一个变量：选择已验证的 MM 候选、固定卡/网络、保持 Wi-Fi 管理链路，确认无通话后执行一次 profile lease 对照。不得同时改变 APN、Initial EPS、backend、普通数据或 SIM 配置。窗口结束必须恢复原 profile 列表、Initial EPS、服务、配置、数据库和所有 receipt，并保存脱敏 P-CSCF/SIP 阶段结果。
