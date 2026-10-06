# MM IMS Exact-Family 租约、SIM 绑定与安全恢复

本文定义 ModemManager（MM）IMS profile/bearer 的归属、生命周期和维护边界。
注册与地址族策略见 [IMS 注册策略](IMS_REGISTRATION_POLICY.md)，实机状态见
[HANDOFF](HANDOFF.md)，采证见 [IMS 诊断](IMS_DIAGNOSTICS.md)，历史过程见
[档案索引](archive/README.md)。这里不保留历史 PID、CID、制品或设备地址作为当前值。

## 1. 目标与范围

MM 有效 `profile-id` 的内置地址族可能优先于请求 `ip-type`；只改请求标签不能改变 profile。
Exact-family lease 在受控路径中严格新建与本次族请求一致的 profile，并证明仅清理自身资源。
它不制造 PCO/P-CSCF，不改变 AKA/SIP/IPsec，也不保证某张卡注册成功。

- 生产固定 `IPv4v6 → IPv6 → IPv4`；双栈仅获一族仍为合法承载。
- 在原 bearer-family 调用内准备 profile，不新增 SIP 无响应后换族重建的外层循环。
- 不恢复旧 `profile_pin_family_conflict` 终止策略来关闭正常强制族兜底。
- 不把过去临时 IPv6-only 避障当作当前配置；全部线路使用完整默认策略。
- `0502395` 已完整撤回 CMCC 专用 flag 补丁与测试，不能声称该问题修复。
  全局 fallback 回归待调查；后续修复和测试采用通用条件，不新增运营商专用逻辑。

## 2. 不可破坏的不变量

1. 同一物理 modem 的 bearer 只由 MM 管理，不借 MM 内部 WDS client，不并行启动 direct-QMI owner。
2. 不覆盖原 profile、Initial EPS、其他线路 context 或普通数据定义；不写固定 CID。
3. SIM logical channel、eSIM profile、MM profile-id、AT PDP CID 是不同对象。
4. owner、SIM、进程、boot、generation、profile、bearer 与网络资源均须有明确绑定。
5. 取消、超时、未知返回、清理失败或身份变化时保留账本，不猜 ID、不盲删、不重放未知写入。
6. profile 创建成功不等于 bearer 成功；IP 地址不等于 IMS 注册；探针不等于正式服务验收。
7. P-CSCF 只能来自同一 retained bearer 的已验证 context、可信配置或 UE-bound DNS。
8. 不因资源阻断清预算、重启基带、全局清 XFRM、停其他线路或偷偷改数据业务。

## 3. 生产接入与隔离架构

- 设备 transport 显式 opt-in；仅标准派生、MM、IMS-only 路径进入 runtime adapter。
- 普通数据开启、catalog 或其他设备仍走原路径，不为启用 lease 自动关闭用户数据。
- 普通路径也持有同设备 flock 至 bearer 准备结束，避免检查空账本后误复用并发创建的 profile。
- 每次族尝试从原 unique owner、已 pin SIM 创建独立接口选择状态，不重新寻找 owner，
  不把上次选中的 `wwan` 接口沿用到下一承载。
- runtime 经 MM Command 的严格 AT 能力校验选择空闲 exact-family profile；
  只有前次完整回收，才允许原计划的下一尝试。不确定结果立即阻断。
- opaque retained handle 转发 SIM、P-CSCF、namespace 操作并携带 profile 生命周期。
- generation/取消谓词直达 CreateBearer/Connect 分发前；已派发请求继续受保护接收结果并最终回收。
- 不调用维护 CLI 或全局 shutdown 来完成生产清理。
- 退出顺序为 SIP/XFRM → retained bearer/网络 → reporting/profile；
  显式 exit 等待 profile 回收，不依赖析构，超时保留账本。

## 4. SIM 绑定校准与 generation

### 库存观测

- 稳定 `line_id` 表示物理槽位，不用 APN/运营商相同推断同 SIM。
- 每轮 registry 在 reader/serving/UE 异步 reconcile 前同步核对全部已知 MM 线路的
  ICCID、SIM/MM object、控制端口和 slot，不让前一线路慢查询延迟后一线路失效。
- 确认变化后关闭新蜂窝 IMS 准入并递增 generation；旧连接、恢复批次、listener 结果不能发布到新 SIM。
- 空 ICCID、未就绪、冲突及 discovery 失败属于未知，不抹掉最后身份或公开 presence/worker，
  不等同于成功查到空库存，不反复清理、不记录 profile 耗尽。
- 已有会话由 retained SIM/owner 校验，不因一次库存未知主动重新附着；
  同卡稳定后仍可按既有策略恢复。
- 新身份连续两轮一致才调度带 revision ticket 的清理；忙碌则下轮重试，不排长队。
  取得 bearer/connect/advance 锁后再次核对 ticket，清理后再核对才重新开放准入。
- 只释放旧自有会话并等待 listener 退出；不向替换 SIM 发旧 SIP 注销，
  不按复用 modem/CID 对新 SIM 回写旧 reporting/profile。
- 普通 MM profile/reporting 属于持久配置，不做通用选择器回滚；
  自有 exact-family profile 只有满足本手册归属证明才可释放。native 原清理策略不变。

### retained bearer 的检查点

- CreateBearer 前两次无缓存 SIM 快照绑定 unique owner、控制端口、SIM object、ICCID、PrimarySimSlot。
- 单卡 `slot=0` 或缺失按 inventory 语义归一到逻辑槽 1；读取失败、类型错误、越界拒绝。
- 派生身份读取前后核对 ICCID；每族请求携带调用方预期 SIM/slot，与 retained 快照比较。
- Connect/监视、IP settings、P-CSCF、初始 SIP 和续期前后继续核对。
- 取消后的 AKA 回调在新认证 IO 前拒绝；SIM/owner 不可验证是终止错误，不继续族轮换。
- reporting 串行锁取得后再次检查 owner/端口/SIM/slot/策略，防止排队时换卡。
- 校准不 Disable/Enable、不修改 Initial EPS、不清一次性恢复预算；
  原 profile/族/P-CSCF 耗尽后仅既有 `primary_ims_recovery.rs` 决定受控重新附着。

## 5. 应用内 eSIM 切换屏障

应用内 enable 在 lpac 之前关闭准入并排空原 IMS；维护 guard 覆盖整个异步操作。
失败/取消也必须等待稳定新观测，不能直接恢复旧映射；重叠切换请求返回冲突。

执行 lpac、身份清空或全局 MM 恢复前必须证明：

- 有有效 MM admission ticket，不能仅凭账本 absence 切卡。
- 同设备持久 profile 账本不存在，旧 Context 已释放设备 flock。
- 全局 bearer、pending-create 及其他相关账本均无残留。
- 不可信目录、损坏文件、权限错误、symlink 或读失败不能当作 absence。

设备 flock 跨 lpac/MM 恢复持有，成功/失败/取消均 RAII 释放；清理不放在 serial permit 内。
全局 MM 操作先取得 registry discovery 独占预留，冻结新发现后再核对库存。
操作结束、最终 registry refresh 前释放库存预留以免自锁；线路 ticket 和设备 flock 仍保留。
多个已知 modem 线路或 slot 冲突保守拒绝；不是任意多 modem 热切换支持。
不增加 eSIM 原流程之外的射频重启，不修改 native 后端控制流程。

## 6. 快照与持久账本

维护 receipt 位于 `/var/lib/simadmin/mm-ims-profile-lease/`，不能仅存在 `/run`：
QMI/AT profile 可能跨进程或重启继续存在。

完整快照包括：

- MM bus unique owner、modem object、物理控制拓扑和稳定 SIM/slot；
- 规范化的完整 ProfileManager 字段，不只 ID/APN/family；
- AT 定义、PDP family/APN、活动 CID、Initial EPS、所有 CID reporting；
- bearer/interface/IP grant 和网络归属；进程身份、boot、generation。

目录/文件使用 flock、权限/归属检查、O_NOFOLLOW、大小限制及原子持久化。
只保存原字段哈希而不保存原 profile 认证口令；receipt 不是配置或数据库备份。
列表排序/序列化变化不算内容变化，字段值变化必须阻断。

- v1 是显式维护 receipt；v2 增加线路哈希、代次、进程启动身份、boot、runtime 阶段、bearer 网络镜像。
- 旧维护 CLI 不采用 v2；runtime 不采用 v1 维护残留。
- 写入前持久化 `Creating`，之后记录已知返回 ID、`Owned`、`RestoringReporting`、`Deleting` 等状态。
- runtime 可记录 bearer pending/cleaning/abandoned 等阶段；不能仅凭顶层 owned/probed 判断可清理。
- 任何原子写入/fsync 失败均阻断后续操作。异常账本保留证据，不手工改 owner/SIM/阶段以解锁。

## 7. 显式创建与释放

以下命令只是接口说明，必须在另行授权的空闲维护窗口执行。

```text
simadmin mm-ims-profile-lease --modem <当前MM对象路径> \
  --device <物理控制设备> --family <ipv4v6|ipv6|ipv4> --apn <已验证IMS APN>
```

默认 `--action inspect`，输出与完整当前快照绑定的 `plan` token。
所有创建要求已验证 QCM410 BAM-DMUX/QMI 拓扑、同 owner/SIM、`IndexField=profile-id`，
主/secondary manager 与 worker 停止，无 bearer/通话/未知 bearer receipt。
创建/释放自身不调用 Enable/Disable/Connect/CreateBearer，不启动服务。

| action | 契约 |
|---|---|
| `inspect` | 只读双快照与 plan |
| `acquire --expected-plan <token>` | ProfileManager.Set 不带 profile-id，严格新建 |
| `acquire-at --expected-plan <token>` | 显式选 MM Command 路径，不与 QMI 自动互相回退 |
| `release` | 仅处理本设备且 APN/family 匹配的自有 receipt |

创建规则：

- QMI 验证设备实际返回 ID、唯一短 tag 和完整读回，不假设固定 ID。
- AT 能力解析器在 MM 与 AT 列表都不存在的受支持 CID 中选择，排除 CID1；
  核验 inactive、写前重读，不因 APN 相同覆盖已有定义。
- 两条路径都验证新项 MM/AT family/APN 一致、原所有字段及 EPS 未变。
- 所有未定义 CID reporting 需为 000，避免新 ID 落到非默认 reporting 而无法证明恢复。
- Set/AT 超时、取消、返回不明或保存失败保留 Creating，禁止再建或猜 ID 删除。
- 只有完整收到明确 QMI 参数拒绝才标记 Rejected；release 双快照证明原状态未变后结案，不发 Delete。

释放规则：

1. 无自有 bearer/网络残留且无通话，精确读回自有 profile 未变。
2. 恢复该 CID 保存的原 reporting（不是一律盲写 000），读回确认。
3. 通过 MM 删除唯一自有 profile，不用空 APN 占位替代删除。
4. 双快照核验原完整 profile/AT/EPS/reporting 恢复后才移除活动 receipt。

reporting/Delete 超时不能重放；未知 owner/SIM/字段变化保守阻断。

## 8. 有界注册探针

`--action probe --expected-plan <当前token>` 只准入 Owned、APN/family 与快照匹配的维护租约。

- 请求前持久化 `Probing`，结束记 `Probed`；取消/崩溃后不再 probe 同一租约。
- 使用已有派生身份、AKA、共享 REGISTER 核心，独立 UE namespace 和内存数据库。
  不启动 Web、调度器、通话/短信监听或生产恢复循环。
- reporting 在 Probing 持久化后经原 unique-owner bus 与串行锁，核验 SIM/静止状态后启用并读回；
  跳过生产核心 mmcli reporting 写入。未知写入/状态保存失败保留 Probing，禁止自动 release。
- 最多一次底层 bearer 建立，强制族错误也不得扩大为第二次激活；不写生产族配置。
- REGISTER 窗口 240 秒，注销另限 40 秒；成功后主动注销，不保持在线服务。
- 注销分别报告 confirmed/already_expired/rejected/access_lost/timeout，
  本地清理成功不代表网络确认注销。
- bearer 回收后停止 worker，仅在自有 namespace 只剩 loopback 且清理确认时删除。
  profile 仍需显式 release。

## 9. P-CSCF 与实际地址族证明

始终验证 profile-id、实际 APN、原 owner/SIM、独占网口和 MM IP grant；不从 AT 配置主机地址。
不能只因 IPv6 前缀相同就借用其他 CID 的 PCO。

原 exact-address / sole-active 证明继续保留；额外多上下文分支仅适用于已验证自有 profile 的 retained bearer：

- MM 实际无 IPv4且授予 IPv6；目标 AT 恰好一行，CID、APN、地址和 /64 授予一致。
- CGPADDR 必须唯一 IPv6 完整地址与目标 CGCONTRDP 完全匹配；
  可以带未授予占位 `0.0.0.0`，不允许真实伴随 IPv4、重复行、错误尾部或不同完整地址。
- 既支持定义为 IPV6，也支持 IPV4V6 但实际只获 IPv6；定义变化或双行歧义拒绝。
- 每个其他活动 CID 独立读 CGCONTRDP，核验 CID、协商 APN、EBI、可用地址族。
  IPv6-capable 定义却无有效 IPv6、显式非 /64 或零 IID 为未知。
- 其他上下文与目标前缀重叠、同 IMS APN、重复 EBI 均拒绝。
  空配置 APN 协商成普通数据 APN只能作排除证据，不借其 DNS/P-CSCF。
- 全部定义/活动表/目标与其他行双快照一致，最后重验 MM 绑定才发布目标自己的 PCO。

普通非自有 pin 不享有此扩展；原 12 秒只读预算、串行锁及 lease guard 不变。

## 10. 清理、对象换代与启动门禁

正常清理先停止新操作，确认 retained owner，再释放 bearer、验证 interface/地址/路由/namespace，
最后恢复 reporting、删除 profile、验证原快照并结案。

- owner 消失或别人占用接口不等于网络已释放；带网络/namespace 的 receipt 保留。
  只有无网络变更的纯旧 bearer receipt 才可能在 owner 消失后遗忘。
- Create 前保存 `.create` intent；外部等待有界，但已派发 Create 在受保护任务内接收晚到对象并清理。
  lease 保存与 Delete 均失败、RPC 不明或进程交接前退出时，intent 阻止再次分配。
- 同 owner 下对象换代：仅向原 unique owner 的 ObjectManager 证明旧 modem/bearer 都不存在，
  只读证明物理网口回主机、旧地址/源路由/私有表规则/namespace 状态消失，再核对对象 absence。
- `UnknownMethod` 字符串不等于对象不存在；不向新 modem 重放旧 bearer 清理。
- profile 清理可在同 owner、同 SIM/slot/物理口、原 profile/EPS 和双快照完全一致时有限重绑定；
  不将 bearer 清理重定向，不接受缺稳定证据的旧 receipt。
- 短暂 MM 换代只在尚未发 reporting/Delete 时有限等待，未知写入不重放。

启动和全部 registry refresh 入口在创建 UE worker/namespace 前处理未结案 v2/恢复事务。
MM/SIM 未枚举时先等待；每轮至多一次只读证明，并有 5 秒冷却，不增加 REGISTER 或基带重试。
恢复不明时不做全局 namespace 搬移，避免先创建新 worker 把空闲恢复条件堵死。

## 11. 无资源旧记录的归档

跨 boot / owner / SIM 变化不是通用自动删除授权。
只有旧资源已不存在且完整安全证明成立，才能归档元数据；仍存在或身份未知继续阻断。

| 入口/情形 | 允许动作 |
|---|---|
| 跨 boot 自动恢复 | 同 SIM/物理拓扑、旧 owner/进程死亡、双库存及所有旧资源 absence 后归档 |
| 跨 owner 自动恢复 | 满足独立恢复准入且旧 profile/reporting absence，可支持同 boot 或换 SIM |
| `inspect-retired` | 只读核验及输出 plan |
| `retire-absent --expected-plan <token>` | 按精确 token 归档已证明无资源的已知 v2 阶段 |
| `inspect-uncreated` / `retire-uncreated` | 显式核验完全未记录创建资源的旧创建意图，仅归档元数据 |

absence 要求：

- 原 owner 用同 bus `NameHasOwner` 确认真正退出，不只是失去 well-known name；当前绑定稳定。
- 原自有 ID 在当前 MM、AT 都不存在；同 ID 即便 APN/family 不同仍视为存在。
- reporting 恢复原值，旧 bearer 镜像、地址/路由/规则无残留，旧 namespace 不存在。
- Creating/Probing/BearerPending、未知 reporting/Delete 不能借普通 absent 入口结案。
- uncreated 专用入口须同 boot/owner/SIM、创建进程已死、原库存完整双快照未变、
  源记录至少静置 120 秒、未记录任何已创建资源且 plan 匹配；自动恢复仍阻断 Creating。

原字节先写入 `retired/absent-<digest>.receipt` 并同步，才移除活动名。
归档后崩溃仅在权限/归属/单链接/完整字节全部匹配时续接；symlink、hardlink、截断或冲突均拒绝。
同步归档目录和父目录，不覆盖历史证据，不清恢复预算。

## 12. 跨 owner 的 present profile 恢复事务

AT 创建无唯一归属标签；相同 CID/字段/指纹无法排除被他人删后同值重建（ABA）。
因此仍存在的跨 owner profile 不可自动认领或删除，只能通过显式批准的精确维护 plan。
不放宽原 `identity_io`、`same_binding`、`release_with`。

### 空闲准入

- 持有设备 flock，仅一个 MM modem，无其他 manager/worker/live Context、pending bearer 或冲突账本。
- 原 owner/创建进程退出；同进程仅允许明确 abandoned 且无 live Context。
- 当前 owner/SIM/slot/boot/物理拓扑稳定，无 MM bearer/通话或旧网络资源。
- 不存在 named namespace，所有进程都在主网络 namespace，排除无名称但仍被持有的 namespace。
- 主 namespace 无 XFRM state/policy；其他租户/容器或无法检查则拒绝。
- 非目标完整库存、EPS、reporting 与源快照一致；目标由账本指定，不固定 CID3。
- present 目标必须有明确 `+CGACT: <cid>,0`；缺行不等于 inactive，分发前再次检查。

程序不会自动停服务、清 namespace、重启 MM/基带或停其他线路来满足条件。

### 命令与一次性日志

```text
simadmin mm-ims-profile-lease --action inspect-stale \
  --modem <当前对象> --device <原物理控制口> --family <原请求族> --apn <原APN>
simadmin mm-ims-profile-lease --action reconcile-stale \
  --modem <同对象> --device <同控制口> --family <同族> --apn <同APN> --expected-plan <plan>
```

`inspect-stale` 不改 modem；首次 present 恢复 plan 缺失/错误，在写事务及发命令前拒绝。
每设备唯一 `.recovery` 绑定源账本原始字节摘要，原 `.json` 不改写。

`Prepared → ReportingDispatched → ReportingConfirmed → DeleteDispatched → AbsentVerified`

- 每条命令前原子写入并 fsync 文件/目录；reporting 最多一次，Delete 最多一次。
- reporting 超时只在精确读回 000 后推进；Delete 超时只在双快照 absence 后推进，present 绝不重发。
- 已批准事务再次调用仅核验结果，不重置写预算；owner/SIM/boot/来源/库存变化使旧 plan 失效。
- 终态才归档原字节及 `retired/reconciled-*.journal`；崩溃收尾须原档完整且摘要匹配。
- 从备份恢复旧账本也不得获得新写预算；孤立/损坏 `.recovery` 阻断普通分配、切卡及其他维护。
- 前端应提示“旧 IMS 资源待核验恢复”，不是运营商拒绝；基带故障优先级保留。

## 13. 验证与设备窗口要求

构建/Rust/注册模拟只在 GitHub Actions 执行，见 [开发指南](DEVELOPER.md)。
本文合并未运行测试；不要把旧部署日志、测试累计数或历史网络结果当作当前 master 通过。

必须覆盖：

- fake/private D-Bus 严格无 ID Set、非固定返回 ID、完整规范化字段、MM/AT 不一致、EPS/reporting 保持。
- 所有持久化失败点、Create/Set/Connect 取消和晚到结果、Delete/报告恢复未知、owner/SIM 更换。
- 未知库存不耗尽恢复、generation/ticket 过期不能发布、slot=0/非法槽、排队 reporting 重验。
- switch drain、flock 跨异步操作、registry 预留、无 ticket 拒绝及失败/取消释放。
- P-CSCF 完整地址、单族授予、歧义/重叠/其他 CID 拒绝，终止错误不能被清理包装吞掉。
- cross-owner ABA、active/缺 CGACT、plan 漂移、防重放、孤立事务、终态归档、原字节不变与备份重放。
- 两套实际 CI 过滤器均执行上述状态机及通用 REGISTER 协议矩阵，核验日志而非只看绿色状态。

设备验收另行授权，先保留管理链路、确认无通话、备份配置/数据库/原账本并制定有界恢复方案。
一次只改变一个变量，不同时改 APN、Initial EPS、backend、普通数据或 SIM。
核对制品 commit/哈希、正式服务注册、requested 与 actual family、P-CSCF 来源和新内核故障。
结束核对原库存/EPS/非目标配置、服务与保护定时器；活跃新租约是正常资源，不按旧记录清除。
探针成功、离线模拟、一次注册、自然续期、通话/音频、换卡及跨 owner 故障注入是独立验收层次。
长通话、普通数据共存、VoWiFi 全生命周期、native 换卡和所有硬件/SIM 不属于本手册的普遍保证。
QCM410 新 fatal 必须停止重复激活，不能盲删账本或操作 remoteproc，见
[QCM410 故障说明](QCM410_BAM_DMUX_MODEM_CRASH.md)。
