# MM SIM / IMS 承载绑定自动校准

本功能是 ModemManager 路径的通用兜底增强，不是 SIM-06 特例，也不是重编号 CID。
适用于稳定物理 line_id 下的实体换卡、应用内/外部 eSIM 切换及 MM 对象重建。
SIM logical channel、eSIM profile、MM profile-id 与 AT PDP CID 是不同概念，本功能不修改 SIM logical channel。

## 生命周期

1. 每轮 line registry 在任何 reader/serving/UE 异步 reconcile 前，先同步核对所有已知 MM 线路的
   SIM ICCID、SIM/MM 对象路径、控制端口和槽位；不会因前一线路慢查询而延迟后一线路失效。
   line_id 保持物理槽位身份；运营商/APN 相同不代表同一 SIM。
2. 确认绑定变化时关闭新的蜂窝 IMS 连接准入，递增 IMS generation，让旧连接/恢复任务失效。
   空 ICCID、SIM 未就绪或冲突只是未知观测：暂停新准入，不抹掉最后已知身份，不在未知状态反复清理。
   已存在的会话另由 retained MM SIM/owner 验证，不因单次库存查询未知就主动重新附着。
   整次 MM discovery 失败也按未知处理，保留公开 presence/worker，不等同于成功查到空库存。
   恢复批次在未知阶段暂停准入，不记录 profile 耗尽；同卡恢复后仍可按原策略自动尝试。
   连接/监听器和恢复批次的嵌套状态更新均携带原 generation，旧任务不能回写新 SIM 的状态或历史。
3. 新身份连续两轮观测一致后，库存监视器调度带 revision ticket 的清理。忙碌时不排长队，下轮再尝试。
   获得 bearer、connect、advance 锁后再次检查 ticket；旧任务不能替新 SIM 完成校准。
4. 清理只释放原自有 IMS 会话/lease；等待旧 listener 退出。不向替换 SIM 发送旧 SIP 注销，
   不按可复用 modem/CID 选择器恢复旧 reporting/profile，不删除未知资源 receipt。
   MM 的 IMS profile/reporting 作为持久配置保留：正常停止、建连中途失败也不按旧 CID 回写，
   避免物理换卡发生在库存轮询间隔或异步失败清理期间时误改新卡。native 原清理策略不变。
5. 清理后再次校验 ticket，失效 SIM 身份缓存，重新开放准入。现有开关、接入优先级、冷却及自动恢复策略
   决定是否重新连接；没有开启 IMS 的线路不会因校准被自动开启。
6. 新连接重新读取当前 PDP 定义并执行已有安全选择规则：匹配项复用，否则在能力范围内选择空闲项；
   不覆盖已有普通数据定义。地址族仍执行原顺序，不固定 IPv6。

应用内 eSIM enable 在执行 lpac 前关闭 MM IMS 准入并清理原 IMS 会话。维护 guard 覆盖整个异步操作；
失败/取消后也要等新的稳定观测，不能直接恢复旧映射。重叠的应用内 MM eSIM 切换请求返回冲突。
本改动不增加 eSIM 原有流程之外的射频重启，也不改 native 后端控制流程。

## MM bearer 校验

QCM410 的 retained MM adapter 在 CreateBearer 前读取两次不缓存的 SIM 快照，绑定原 unique owner、
控制端口、SIM object、ICCID 和 PrimarySimSlot。单卡 MM 的 slot=0/缺失属性按 inventory 语义归一为
逻辑槽 1；D-Bus 读取失败、属性类型错误或越界槽值均拒绝。派生身份读取前后核对 ICCID，
调用方的预期 SIM/槽位随每个族请求传入，在 Create 前与 retained 快照比较；连接/监视、IP settings
与 P-CSCF 读取继续核对该绑定，初始 SIP 和续期前后再核对。取消后的 AKA 回调不能提交新认证请求。

SIM/owner 变化或不可验证是终止性失败，不以地址族轮换继续使用旧绑定。
P-CSCF 发布仍严格检查 bearer/profile、实际 APN、IP grant、唯一接口及上下文归属；不能仅凭前缀相同
把另一个 CID 的地址借过来。未知/失效绑定只通过 retained provider handle 清理。

若新 SIM 的正常 profile/地址族/P-CSCF 兜底耗尽，仍由既有 `primary_ims_recovery.rs` 决定是否允许
一次受控重新附着。reporting 写入获得串行锁后再次核对原 owner/端口/SIM/槽位和当前策略，防止排队期间
换卡后写旧 CID。校准本身不调用 Disable/Enable，不清除一次性预算，不修改 Initial EPS 或普通数据。

## 失败所有权与保守阻断

- MM 正常停止和失败只让 retained provider 处理自己的网络、命名空间和 bearer；不执行通用选择器清理。
- owner 消失或另一 bearer 占用接口时，不据此认定网络已释放。带 network/namespace 的原 receipt 保留，
  后续恢复保持阻断，需要独立人工核验；无网络变更的纯旧 bearer receipt 才可在 owner 消失后遗忘。
- **同一 MM owner 下的 modem/bearer 对象换代**：新连接前的 `recover_owned` 可能被旧 lease 的清理 RPC 阻断，并非新连接缓存了旧ModemBinding。旧对象清理失败后，只向原 unique owner 的 ObjectManager 查询，要求旧 modem 与 bearer 同时不存在；只读确认原物理网口已在主机、旧地址/源路由/私有表规则与原namespace状态均已消失，然后再次核对对象缺失，才结案自有receipt。
  不把`UnknownMethod`字符串直接等同对象消失，不向新modem重放旧清理，不因owner变化/观测失败/状态残留丢弃记录。此路径不改profile/地址族兜底或恢复预算；验证状态见HANDOFF。
- Create 前保存 `.create` intent；对外等待有界，但已派发的 Create 在受保护任务中继续接收结果，晚到对象仍须清理。
  已知新对象的 lease 保存及 Delete 都失败、RPC 结果不明或进程在交接前退出时，intent 留存并阻止再次分配。
  不自动猜测未知对象的所有权、不向新 MM owner 重放清理；这不是未知孤儿资源自动恢复。

## 验证范围

- Rust 状态机：同槽实体/eSIM 身份变化、未知观测、快速多次变化、旧 ticket、对象变化、槽位冲突、线路隔离、native 不介入。
- Rust runtime：generation 取消、旧注册/续期/批次结果无法发布、未知库存不锁死自动恢复、维护 guard 失败退出和重复切换准入。
- 隔离 D-Bus：相同 SIM object 更换 ICCID、单卡 slot=0、调用方 SIM 不匹配、IP/P-CSCF 读取期间换卡拒绝发布、
  排队 reporting 的 SIM/策略重验，以及 owner 丢失保留网络 receipt、Create intent 保留。
- Rust helper/mock：非法槽值、清理错误包装不丢失终止标志、族循环只尝试一次、旧认证回调在 IO 前被拒绝。
- Python 守卫：库存入口、锁/ticket 接线、不执行射频或 CID 写入、两套 CI 的测试过滤器。

Rust 仅在 GitHub Actions 编译运行；具体构建及实机结论以 [HANDOFF](HANDOFF.md) 的最新记录为准。
硬件无关测试不能代替真实 eSIM/实体换卡验收。未在健康的 SIM-06 会话上自动换卡、清预算或强制重新附着。
VoWiFi 独立会话/缓存全生命周期及 native 换卡迁移不属于本次已实现承诺。
