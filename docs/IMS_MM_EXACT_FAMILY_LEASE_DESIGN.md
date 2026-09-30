# MM IMS Exact-Family Profile Lease Design

> 状态：2026-09-30显式维护入口已完成代码/两套CI/双架构，并在设备验证MM-AT安全新建与删除闭环（6a77d92）；QMI Set创建请求被设备拒绝的事实保留。未接入自动注册回退、未进行临时profile上的SIP注册，不能把维护入口完成等同于IMS修复。
> 目标：解决 ModemManager 中有效 `profile-id` 优先于请求 `ip-type` 的约束，同时保持单一 MM owner、UE namespace 隔离和可核验恢复。

## 显式维护入口（代码/CI及 profile 生命周期已实机验证）

`simadmin mm-ims-profile-lease --modem <MM对象路径> --device <控制设备> --family <ipv4v6|ipv6|ipv4> --apn <派生IMS APN>` 默认`--action inspect`，输出与当前完整快照关联的`plan` token。
`--action acquire --expected-plan <token>` 调用不含profile-id的ProfileManager.Set；`--action acquire-at --expected-plan <token>` 是明确选择的MM Command路径，先查询AT能力/活动与双快照，只在不存在的受支持CID上定义profile。二者不自动相互回退。
`--action release`只处理该设备的自有receipt，要求APN/family与记录相符；只有自有配置读回一致、无承载/通话、恢复原reporting后才通过MM删除并验证。

- 只接受已验证QCM410 BAM-DMUX/QMI拓扑、同MM unique owner与SIM、IndexField=profile-id。要求两套主程序均已停止、无bearer/通话或未知bearer receipt；不调用Enable/Disable/Connect/CreateBearer，也不启动服务。
- acquire前读取两次完整MM profile字段指纹、AT定义行及PDP族/APN、Initial EPS、所有CID reporting。QMI路径不传ID、核验设备返回ID及唯一短tag；MM-AT路径由同一安全能力解析器选择在MM与AT两套列表中均不存在的CID，排除CID1，验证不活动并写前重读，绝不因APN相同覆盖旧定义。两条路径均核验新profile的MM/AT族与APN一致、完整原条目未变，不写死4。
- QMI profile可能跨重启存在，因此元数据receipt保存在`/var/lib/simadmin/mm-ims-profile-lease/`而非仅/run；写前Creating、已知返回ID、Owned、RestoringReporting、Deleting状态均持久化，使用flock/O_NOFOLLOW与目录权限/文件校验。receipt仅保存原字段哈希，不保存原profile认证口令；不是配置/数据库备份。
- 新ID可能落入任一空闲位置，因此试验前要求所有未定义CID的reporting均为000；不假定删除能恢复非默认reporting。执行release时若该自有CID被应用设成111，先恢复其确切原值，再删除自有profile并验证原完整列表/AT/InitialEPS/reporting恢复；不写回已有普通profile。
- Set或AT写入超时/取消/返回值不确定或存储失败保留Creating，禁止新建/猜ID删除。只有完整收到QMI Create的指定参数拒绝才记录Rejected，随后显式release必须两次证明原始快照未变，才结案且不发Delete。报告恢复与Delete超时不重复写；未知owner/SIM/字段变化仍失败关闭。
- **此入口不改变现有自动CID选择、profile来源大兜底或地址族顺序，也没有偷偷增加“无响应再新建profile”的生产流程。** 下一阶段用已通过CI的维护能力进行一次独立profile对照，再根据证据决定生产集成；当前不能声称已经注册。
- 单元/mock及private-D-Bus测试覆盖无ID Set、非固定返回ID、MM/AT不一致、原字段/owner/SIM变化、每次持久化失败、Set取消、报告恢复和Delete不确定结果、状态回收和身份字段不进入receipt。Rust只在Actions运行。

## 有界注册对照入口（续接实现，尚待 CI 与实机验证）

新增显式 `--action probe --expected-plan <当前 inspect token>`，只准入 `Owned` 租约且 APN/family 与完整快照一致。发请求前持久化 `Probing`，结束后记为 `Probed`；取消或崩溃后不能再次 probe 同一租约。

- 复用现有派生身份、AKA、REGISTER 核心；独立 UE namespace、内存数据库，不启动 Web 服务、调度器、通话/短信监听或生产恢复循环。
- capability 校验实际 CID/APN/MM 端点与族，最多调用一次底层承载建立；强制单族错误不能借此对同一 profile 再激活。REGISTER 核心窗口 240 秒，注销另限 40 秒；成功报告仅表示本次注册成功，随后主动注销，不是维持在线服务。
- 注销结果分别报告 confirmed/already_expired/rejected/access_lost/timeout，不把清理成功当作网络已确认注销。承载回收后停止 worker，仅在自有 namespace 只剩 loopback 且清理已核验时删除；profile 仍须显式 release。
- 同一 MM owner 内对象重新枚举，只在旧 modem 确认消失、物理控制口及稳定 SIM/slot、原 profile/EPS 全匹配且两次快照一致时衔接 profile 清理；不把 bearer 清理重定向到新对象，不接受旧 receipt 缺失稳定归属证据的换代。
- 此节描述待验证代码，不是已部署、已注册或已完成生产集成的证据。原 6a77d92 的 profile 闭环证明保持独立。

## 维护工具的验证事实（2026-09-30）

- `c71bee2`：两套CI/39累计新增与更新回归+8兼容/双架构核验通过，设备inspect成功；QMI Set请求返回`Couldn't create profile: DS profile error: invalid-parameter-length`，原3项profile和完整快照token未变。
- `b83a0f4`：tag由44字节缩至16字节、增加明确拒绝状态；两套CI/41累计回归+8兼容/双架构通过，但设备仍同样拒绝。因此**不能认定只是名称过长，也不能泛化成所有QMI Create均不支持**。
- 首版Creating未存明确拒绝分类；现场经原MM日志唯一tag及明确错误、同owner/进程、完整快照一致验证后，将原记录归档为`.rejected`保留，未删profile/预算。新版Rejected记录由工具两次核验原状态后结案。
- `6a77d9278e5d8fbaf4519fa0c77dfc2a6a5c7835`：新增显式MM-AT创建。Validate36722549817/Build36722549812全部success，下载两套日志核实44累计新增/更新回归+8兼容检查；ARM64/AMD64制品digest/meta/ELF/程序和前端均已核验。
- **13:43 UTC实机闭环成功**：inspect→acquire-at，自动选空闲CID4创建`IPV4V6/ims`，MM+AT读回/原profile/EPS/reporting检查通过；随后release只删除本次自有profile，最终inspect恢复3项、无pending，token与最初完全相同。没有承载激活或REGISTER，未碰CID1/2/3定义。
- 工具只在`/opt/simadmin-staging/ims-profile-maintenance-6a77d92/`运行，未替换正式cf13a66或启动主服务/beta8/secondary；MM PID48819未变，recovery timer恢复active。设备recovery service为`oneshot + RemainAfterExit=yes + active/exited + MainPID0 + Result=success`，是已结束检查，不误判为正在恢复；一次预检误拒绝记录保留，未发写操作。
- 证据`.local/evidence/ims-route-completion/{c71bee2,b83a0f4,6a77d92}/`与`profile-lease-contract/`。**下一步仍是有界注册对照**，需要考虑承载清理引起MM对象换代与临时profile的安全回收；不因profile闭环成功就宣称当前eSIM已注册。

## 2026-09-30：同机 beta8 成功带来的新证据

- 用户运行同哈希beta8/930365d成功；MM日志明确profile4先IPv6失败，再IPv4成功并有RX。停止后profile4不存在，原profile3 IPV4V6/ims保持。当前cf13a66用profile3双族配置，SIP无响应。
- MM1.24.0的`load_settings_from_bearer`/`get_profile_ready`依旧以profile内部族驱动WDS，不是只有1.18才有该语义。当前APN匹配复用不检查每次尝试的PDP族。
- 不把profile4视为固定答案；需要严格新建、取得实际返回ID，或复用可证明属于本功能的exact-family定义，不覆盖任何既有条目。
- **只在原bearer族循环内补profile准备，仍可能无法修复这张卡**：dual建立取得IPv4就提前返回成功；后续SIP失败不继续该bearer循环。本次成功beta8的IPv6/IPv4是分别建立的MM承载。因此必须先对照“新profile”与“单族profile”各自作用，不以连接标签变化冒充等价测试。
- 原地址族顺序IPv4v6→IPv6→IPv4、profile来源大兜底、安全/费用保护保持。若最终需要将SIP阶段的无响应交回按族承载重建，应单独明确授权与有限预算，不在这个准备层偷偷增加另一套重试。
- 用户已停beta8及主服务，保持停机现场；后来已批准并执行上述临时profile新建/回收闭环，但**尚未在该profile上做SIP注册对照**。beta8测试前MM重启/secondary停止亦是混杂变量，不能据此要求复现这些操作。
- 下方旧设计曾建议`profile_pin_family_conflict`终止，后续70dfe3d已撤回该生产行为；既有正常forced-family兜底不能因实现本方案被关闭。设计实施应以现代码/最新HANDOFF为准。

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
