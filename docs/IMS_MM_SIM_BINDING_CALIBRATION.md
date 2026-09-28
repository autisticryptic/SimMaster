# MM SIM / IMS 承载绑定自动校准

本功能是 ModemManager 路径的通用兜底增强，不是 SIM-06 特例，也不是重编号 CID。
适用于稳定物理 line_id 下的实体换卡、应用内/外部 eSIM 切换及 MM 对象重建。
SIM logical channel、eSIM profile、MM profile-id 与 AT PDP CID 是不同概念，本功能不修改 SIM logical channel。

## 生命周期

1. 每轮 line registry 发现结果在异步 UE reconcile 前核对 SIM ICCID、SIM/MM 对象路径、控制端口和槽位。
   line_id 保持物理槽位身份；运营商/APN 相同不代表同一 SIM。
2. 确认绑定变化时关闭新的蜂窝 IMS 连接准入，递增 IMS generation，让旧连接/恢复任务失效。
   空 ICCID、SIM 未就绪或冲突只是未知观测：暂停新准入，不抹掉最后已知身份，不在未知状态反复清理。
   已存在的会话另由 retained MM SIM/owner 验证，不因单次库存查询未知就主动重新附着。
3. 新身份连续两轮观测一致后，库存监视器调度带 revision ticket 的清理。忙碌时不排长队，下轮再尝试。
   获得 bearer、connect、advance 锁后再次检查 ticket；旧任务不能替新 SIM 完成校准。
4. 清理只释放原自有 IMS 会话/lease；等待旧 listener 退出。不向替换 SIM 发送旧 SIP 注销，
   不按可复用 modem/CID 选择器恢复旧 reporting/profile，不删除未知资源 receipt。
5. 清理后再次校验 ticket，失效 SIM 身份缓存，重新开放准入。现有开关、接入优先级、冷却及自动恢复策略
   决定是否重新连接；没有开启 IMS 的线路不会因校准被自动开启。
6. 新连接重新读取当前 PDP 定义并执行已有安全选择规则：匹配项复用，否则在能力范围内选择空闲项；
   不覆盖已有普通数据定义。地址族仍执行原顺序，不固定 IPv6。

应用内 eSIM enable 在执行 lpac 前关闭 MM IMS 准入并清理原 IMS 会话。维护 guard 覆盖整个异步操作；
失败/取消后也要等新的稳定观测，不能直接恢复旧映射。重叠的应用内 MM eSIM 切换请求返回冲突。
本改动不增加 eSIM 原有流程之外的射频重启，也不改 native 后端控制流程。

## MM bearer 校验

QCM410 的 retained MM adapter 在 CreateBearer 前读取两次不缓存的 SIM 快照，绑定原 unique owner、
控制端口、SIM object、ICCID 和 PrimarySimSlot。创建/连接/监视、IP settings 与 P-CSCF 读取核对该绑定。
上层还核验 retained bearer 的 SIM 与派生配置使用的线路 SIM 相同，防止在库存轮询间隔内错接新 SIM。

SIM/owner 变化或不可验证是终止性失败，不以地址族轮换继续使用旧绑定。
P-CSCF 发布仍严格检查 bearer/profile、实际 APN、IP grant、唯一接口及上下文归属；不能仅凭前缀相同
把另一个 CID 的地址借过来。未知/失效绑定只通过 retained provider handle 清理。

若新 SIM 的正常 profile/地址族/P-CSCF 兜底耗尽，仍由既有 `primary_ims_recovery.rs` 决定是否允许
一次受控重新附着。校准本身不调用 Disable/Enable，不清除一次性预算，不修改 Initial EPS 或普通数据。

## 验证范围

- Rust 状态机：同槽实体/eSIM 身份变化、未知观测、快速多次变化、旧 ticket、对象变化、槽位冲突、线路隔离、native 不介入。
- Rust runtime：generation 取消、旧注册结果无法发布、维护 guard 失败退出和重复切换准入。
- 隔离 D-Bus：相同 SIM object 更换 ICCID、调用方 SIM 不匹配，以及 P-CSCF 读取期间换卡拒绝发布。
- Python 守卫：库存入口、锁/ticket 接线、不执行射频或 CID 写入、两套 CI 的测试过滤器。

Rust 仅在 GitHub Actions 编译运行；具体构建及实机结论以 [HANDOFF](HANDOFF.md) 的最新记录为准。
硬件无关测试不能代替真实 eSIM/实体换卡验收。未在健康的 SIM-06 会话上自动换卡、清预算或强制重新附着。
VoWiFi 独立会话/缓存全生命周期及 native 换卡迁移不属于本次已实现承诺。
