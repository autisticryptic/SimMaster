# 原生后端：架构、能力与安全契约

> 默认后端仍是 ModemManager（MM）；native 是显式实验选项，不等于已经完整替代 MM。
> 本文是 native 发现、AT 事件、短信、SIM 通道、资源恢复与专项维护的统一说明。
> 当前开发优先级见 [开发计划](DEVELOPMENT_PLAN.md)，现场状态见 [HANDOFF](HANDOFF.md)；
> 分阶段提交、失败过程与原始验证记录保存在 [历史档案](archive/README.md)。

## 1. 状态与证据层次

| 范围 | 已实现 | 尚不能据此宣称 |
| --- | --- | --- |
| 后端与控制器 | 显式选择、物理操作门/flock、端口归属与代次检查 | 同机不同 modem 的 MM/native 混合 owner 已接通 |
| 协议与承载 | QMI DMS/NAS/UIM/WDS、MBIM、AT、SIM/APDU、UE 接口归还 | 所有协议组合、型号和固件均可用 |
| 发现与诊断 | 被动 sysfs 扫描、脱敏 backend/SIM 通道状态 | 候选端口可用或设备已经由 native 接管 |
| AT 与短信 | 单读者、URC 分流、私有 inbox、逐片发送及送达关联 | 实网短信、电话、USSD 或通知恰好一次已验收 |
| 生命周期 | 持久化 owner/代次、释放终态、显式恢复与专项维护 | 未知孤儿资源可自动清理或设备复位即可解锁 |
| 安装与运行 | 依所选后端装配依赖与恢复资源 | 1.1.5/1.1.6 完整发布矩阵通过 |

native 功能证据检查点为 `302b70e`：Validate `36003900244`、Build `36003900240`
成功，双架构构建成功，Publish skipped；包含此前短信与恢复增强。此检查点不是当前仓库 HEAD。
已有实机**只读发现**证据，仍无 native 端到端接管验收通过记录；MM 实机结果不能移作 native 证据。
代码、自动化/CI、硬件/运营商验收、发布是四个独立状态。

## 2. 控制器、身份与准入

```text
API / 线路服务 / 自动化 / IMS、VoWiFi / UE worker
                         │
               统一设备能力与稳定线路绑定
                         │
               单一物理设备控制器 / driver
                 ├─ MM provider（默认）
                 └─ native provider（实验）
                      ├─ QMI adapter
                      ├─ MBIM adapter
                      └─ AT session
```

- QMI、MBIM、AT 是可组合的底层适配器，不是几个独立 owner；一台 modem 的全部端口、
  槽位、SIM 鉴权及业务由同一物理控制器协调。进程内操作门与跨进程 flock 同时生效。
- 旧配置保持 MM。native 必须同时设置 `mode: native`、`allow_unvalidated_native: true`，
  并提供明确设备配置；未知目标不自动回退 MM、不选第一个 modem、不猜端口。
- 当前选择仍为全局二选一；native 要求 MM daemon 未运行，应用不会替操作者停止 MM。
  同机不同 modem 分别由 MM/native 管理是后续目标，不是现有混合部署能力。
- 后端交接必须有独立维护窗口：无活动通话，旧 owner 已释放 bearer、SIM 通道、UE 接口及归属。
  不热切注册、自然续期或通话中的会话；失败不争抢式切到另一后端。
- `line_id` 表示稳定物理槽锚点/UIM slot，或独立读卡器线路；SIM 覆写另随 `SimBindingKey`。
  `/Modem/N`、`/Bearer/N`、CID、网口编号和 USB bus/dev 编号都不是持久身份。
- 控制面可留宿主；IMS 数据面强制进入对应 per-UE worker/netns。接口须证明独占，
  不接管管理网络/其他线路，也不退回宿主网络制造注册成功。
- QCM410/QCA410 的设备契约是**主 QMI 承载 IMS、DATA6 承载普通数据**；不恢复旧反向布局，
  不将此布局推广到其他设备。未知 driver 明确报告能力不足。
- `GET /api/modem/backend` 用于查看所选后端、设备能力和脱敏资源状态，不是接管入口。
  supported、unsupported、unknown 必须区分；控制成功不等于驻网、IMS 或业务成功。

## 3. 射频意图与承载生命周期

- 发现/控制、射频、蜂窝驻网、普通数据、IMS/VoWiFi 是不同层次；`data=false` 不等于未驻网。
  飞行模式开关是保存的意图，实际未知/过渡/失败必须独立显示。
- 射频与 bearer 操作在门内重读最新意图及代次。进入飞行模式先释放普通数据和蜂窝 IMS，
  不清除 VoWiFi/Trunk 意图；退出飞行模式不自动重开 data/IMS。
- VoWiFi 准备/停止不能偷偷开启射频。某硬件在 RF 关闭时无法访问 SIM，应报告能力限制。
  显式定时流量可在持久 data=false 时运行，但仍服从线路、飞行和漫游限制，结束按最新意图恢复。
- API、自动化、短信和通话不能绕过费用限制；新回退执行前重新准入，详见
  [IMS 注册与资费策略](IMS_REGISTRATION_POLICY.md)。native 不能冒充已提供 MM 专属的非漫游证明。
- QMI/MBIM 会话、地址、路由、网口移入/归还均须绑定 owner 和控制代次。
  取消、超时、清理失败保留 receipt；不能清理新实例或其他 owner 的资源。
- 每个确认释放的 CID/session 立即从内存待清理列表移除并持久化；即使网口仍待归还，
  后续清理也不重发旧数字 ID，持久化失败不把已释放 ID 加回来。
- native 启动不沿用 MM 的全局 stranded-link 自动清扫；不明确归属时阻断，不无限重试。
- 应用启动后的射频门不保证从上电起零射频。固件/NV、内核、MM/NM 或外部 profile 的
  启动副作用与冷启动离线策略仍需设备级审计；去掉 MM 也不自动消除这些行为。

## 4. 被动发现：不接管、不探测协议

```sh
simadmin discover-native
simadmin discover-native --sys-root /snapshot/sys --dev-root /snapshot/dev
```

只扫描 sysfs，不打开设备节点、不发 AT/QMI/MBIM、不连接 D-Bus、不修改配置或停止 MM；
可在 MM 运行时使用。输出 JSON 数组，无可识别 modem 为 `[]`，根目录不存在报错。
这是单次快照，不是热插拔监控；拔插期间缺项应重新扫描，不能触发自动修复。

| 输出 | 使用约束 |
| --- | --- |
| `sysfs_anchor` | USB 物理设备、PCI function 或 SoC WWAN 父设备 |
| `hardware_key` / `line_id` | 根据 sysfs 与默认 slot 1 派生的建议值；须与 `inspect-modems` 的物理键和实际 slot 核对，MM 自定义 UID 可能不同 |
| `usb_generation` | `busnum:devnum` 观察值，不是跨重启身份或 receipt 代次 |
| `qmi_controls` / `mbim_controls` / `serial_ports` | 内核候选，不证明协议可用；激活仍核验字符设备、物理归属和 owner |
| `at_port_hint` | 仅角色提示；多口不任意选择，候选配置永不自动填写 AT 口 |
| `net_interfaces` | 当前宿主可见、同物理祖先网口；已在 UE namespace 的接口可能缺席，不按列表顺序分配 IMS/data |
| `candidate` | 仅 QMI/MBIM 控制口及协议唯一时生成不完整 devices 条目；`at_device`/`ims`/`data` 为空、短信存储消费关闭 |
| `issues` | 缺口、未绑定驱动、未确认 AT、物理键/slot 或多端点歧义 |

纯 AT、多控制口/协议只报告观察结果。AT 口须在独占维护窗口确认后补齐；不自动写 `new_id`、
切 USB 模式、重启设备或猜 QMAP/BAM-DMUX 映射。发现不读取 SIM 或输出 SIM 身份，
但设备名和拓扑仍是本机诊断信息，分享前须检查。

## 5. AT 会话与事件

- 命令、USSD、短信提交和被动读取共用 `at_session` 会话互斥锁，不另起抢读字符设备的线程。
  所有 IO 沿物理操作门执行，并在前后核验端口与代次。
- 已知 URC 与无关命令响应分流；同名注册查询响应仍归查询。拨号/接听的 `NO CARRIER`、
  `BUSY`、`NO ANSWER` 是终结码，异步挂断不能中止无关的 `AT+CSQ` 查询。
- 命令前排空、命令执行和短信提交都保留完整 URC。CMGS 的 `>` 仅在帧边界识别，
  不因 USSD 文本含大于号误触发；未知同名响应不靠猜测归属。
- 单帧最多 32 KiB、普通响应最多 1 MiB、每次排空/事件读取最多 64 次有界读；
  命令仍有截止时间，噪声不能无限占用内存或物理门。
- 公共 URC 仅投影为七个可合并 bool 提示；64 项有界广播唤醒通话/驻网权威核对。
  消费者落后收到 Lagged 后重新核对，不能将提示视作接通或注册成功。
- 存在且启用的 native 线路每秒运行同一个有界 AT pump；只读显式 AT 口，QMI-only 不猜串口。
  无/禁用线路不运行；语音/驻网观察独立于短信接收开关。
- 被动 `AtPoll` 不发送 AT 命令，但打开口仍需 termios 配置。空闲通话每 15 秒兜底核对，
  活动通话保留快速结束判定，权威状态仍为 `CLCC`；驻网/线路每 60 秒兜底，MM 调度不变。
- Web 仅发布 `modem.native_observation` 的 bool 提示或丢失计数，不广播号码、正文、USSD、
  PDU 或 SIM 身份；直接 PDU 另走私有队列。IO 错误不触发短信重发、改 APN 或未知资源恢复。

## 6. 短信：持久化先于确认

### 接收与 SIM 作用域

1. 线路须 present/enabled，设备 `sms_reception_enabled`、线路接收策略和 IMS/CS 接收权均准入。
   IMS 取得接收权时 native 默认暂停；禁用短信接收不消费 modem 存储。
2. 在同一物理门内重核 QMI primary slot 与 SIM，绑定稳定 SIM 摘要，再初始化 PDU 模式；
   返回可消费 scope 前再次核验。换卡/槽位变化保留原 inbox，不跨卡投递。
3. `+CMT`/`+CDS` 校验 SMSC/TPDU 长度与类型，最多缓存 16 个私有 PDU。
   每片先提交 SQLite inbox，成功后才允许 CNMA 或 modem 存储删除，不等全部分片才确认。
4. 完整消息、SIM-scoped 去重、`sms.received` 事件和 inbox consumed 状态在同一事务提交，
   然后才接入现有广播、trunk bridge 与通知。

普通 MT 仍优先 `AT+CNMI=2,1,0,1,0` 存储通知，不主动启用 direct-only；保留 15 秒完整扫描。
存储 PDU 同样入 inbox，删除前重核 SIM、索引和精确 PDU。CMGL 不支持、删除失败或新提交受限，
不阻止重放已提交且 scope 安全的记录；未提交 token 不 ACK、未确认内容不删除。

### CNMA 与容量边界

- 先读 `AT+CSMS?`；只有 service=1 且 MT 支持明确才考虑 `AT+CNMA=1`，service=0 不需要，未知不猜。
- CNMA 无消息 ID，只接受唯一、完整、当前 SIM/会话、无丢帧且收到不超过 10 秒的 token。
  多待确认、溢出、分帧错误、旧连接或超时均拒绝盲 ACK；写前退休 token，失败不重发。
- 丢帧/未知 service 形成的 ACK fence 不能自动清标志，须重新建立确认过的接收会话。
  10 秒是本地保守上限，不保证满足所有固件的 ACK 时限。
- 私有表 `native_sms_inbox`、`native_sms_received` 位于应用 SQLite 数据库。
  待处理/隔离 PDU 每 line 最多 256、全库 4096；满时拒绝新提交且不 ACK，不淘汰未消费数据。
- 每 line 保留最近 2048 个完成指纹，完成后清除原始 PDU。SIM scope 含物理 key、slot 与身份摘要。
  分片按 SIM、发送者、PID/DCS、UDH（8/16-bit reference、端口 IE）、reference/total 和最多
  5 分钟 SCTS 窗口归组；重复幂等，冲突/缺片不拼接，无效/截短 PDU 隔离。

### 发送与送达

- 首片前持久化 outgoing pending 和预期总片数；物理门与独立任务保护整个发送，HTTP 取消不丢账本。
  native SUBMIT 设置 TP-SRR，共享 IMS RP-DATA 编码不变。
- 逐片保存实际 `+CMGS: <mr>[,<ackpdu>]` 和发送时间区间。任一结果/提交不明即
  `submission_state=unconfirmed`，保留 pending 与已提交前缀；UI 提醒待确认，不转其他路径自动重发。
- 报告须匹配 line、SIM、收件人、实际 MR，且 TP-SCTS 在该片发送区间窄容差内；
  不猜号码格式等价，多候选/未知提交不标送达，报告先到则保留待关联。
- 仅 TP-ST=0 算确认送达，全部预期分片通过才将状态改为 `delivered`。
  后来失败/临时报告不降级已确认成功；SMS-COMMAND 报告不关联 SMS-SUBMIT。
- 已确认发送历史保留 7 天，未确认记录不自动删除；达到上限阻止继续盲发。
- 旧 MM/IMS 记录缺可靠 SIM scope，不能按号码/正文/时间丢弃另一张卡的消息。
  native 与旧 IMS 的完全统一去重、通知/trunk 崩溃重放不在已实现范围；入库不等于外部恰好一次送达。

## 7. SIM/APDU 通道与持久化账本

`GET /api/modem/backend` 的 `sim_channels` 只暴露用途、slot、本项目确认的 client/channel ID、
状态、计数与待核对标志。`capacity=null` 表示缺设备证据，不能以当前分配数冒充最大容量。
不记录 AID、APDU、SIM 身份、RAND/AUTN、AKA 材料、会话密钥或 APN 密码。

- 完整 open/APDU/close 在同一物理门内，eSIM 与 IMS AKA 不可交错。
  AT CCHO/CGLA/CCHC 在分配前写意图；QMI UIM 在 **CTL client 分配前**写意图。
- QMI open/close 核验 service/client/transaction/message，indication 不能充当关闭确认。
  明确协议拒绝可结案 open-pending；超时、损坏回复或未知分配保留 receipt。
- AT 仅确认本次 channel 关闭后移除记录；QMI channel close 只更新状态，CTL client release
  成功后才清除。Drop 不盲发 CCHC；已知 ID 落盘失败仍尝试关闭已知通道。
- lpac 同样持物理门，显示 `purpose=esim`；helper 内部通道不可见，标记
  `external_channels_unknown=true`，不虚构计数。正常完成释放范围，错误/超时保留待核对记录。
- 未解决 SIM receipt 阻止本进程后续 SIM 分配和专项维护；新 native/MM 启动也不能接管。
  MM 路径不使用 native 通道账本；不是所有型号的通道泄漏自动修复器。

通用 schema-2 receipt 在 `/var/lib/simadmin/native-control/session-*.json`，目录 0700、文件 0600；
`/run/simadmin/native-control/` 保留 flock 与旧账本兼容检查。备份须包含持久目录，不只应用数据库。
envelope 记录 boot ID、PID/起始 tick、物理 key/slot/sysfs 锚点、控制节点 canonical/sysfs/rdev/inode、
代次摘要、操作载荷和 `cleanup_confirmed`。创建、替换、清除同步文件与父目录并核验原 owner/代次。
正常清理先持久化 `cleanup_confirmed=true` 再删除，供“已释放、未删记录”恢复。
启动检查两目录的全部 pending，包括其他 line、DJI、损坏记录及 `.PID.tmp`；改 key/slot 不能绕开。

## 8. 显式恢复：只归档已确认清理的记录

恢复入口在数据库、native fleet、UE worker、namespace 扫尾及服务启动之前运行：

```sh
simadmin native-recovery
simadmin native-recovery --receipt session-<line-id>-sim.json --config /path/to/config.yaml
```

默认只列元数据，不打开 modem/连接 D-Bus；单文件计划绑定 receipt 字节摘要、当前控制代次、
原 owner 存活状态和配置物理归属。仅 `eligible=true` 才可在独立维护窗口显式执行：

```sh
simadmin native-recovery --receipt session-<line-id>-sim.json --config /path/to/config.yaml \
  --apply --expected-revision <完整revision> \
  --confirm-line-id <line_id> --confirm-physical-key <physical_key>
```

执行重新核对计划、原/新端口及物理 flock、MM 未运行，再写 resolution 证据，将精确原记录归档到
同目录 `resolved/`，不覆盖不同内容的档案。它不发协议命令、不重启设备、不移动网口、不启动服务；
之后须另行显式启动并取得新租约。跨进程/控制代次/宿主重启仅在已确认完整清理且归属可核验时可结案。

以下继续阻断：未知分配/关闭/复位结果；schema-1/未知/损坏/部分记录；存活 owner 或占用 flock；
MM 在运行、过期 revision、物理目标不明；尚存网口/namespace 归还义务或未确认固件会话。
helper 退出、设备重插或“已重启”都不是固件资源释放证明；控制代次也不等于所有型号的固件 boot ID。
DJI 专用旧格式不套用通用恢复证明。没有 `--force`、批量清账、旧 CID/session 重放或自动孤儿接管。
MM IMS profile lease 是另一套账本，见 [MM 租约设计](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)，不能混用证明。

## 9. Quectel 显式维护

已实现 EC20/EC25/EG25 的认证线路接口：

- `GET /api/modem/lines/{line_id}/native/quectel/diagnostics`
- `POST /api/modem/lines/{line_id}/native/quectel/plan`
- `POST /api/modem/lines/{line_id}/native/quectel/apply`

仅适用于已存在、显式 native 独占线路；不是发现/注册重试自动动作。诊断通过确认的 AT 口读取
CGMM、QCFG ims/usbnet/usbcfg、QMBNCFG AutoSel/List；缺失/拒绝明确报告，未知型号不发送设置。
这里“只读”是不改 modem 配置，不是被动 sysfs 扫描。

先关闭 IMS/data intent、释放 bearer/通话，再计划动作：`set_ims` 0/1/2（MBN 默认/启用/禁用）、
`set_usb_network` 0/1（EC2x QMI/RMNET / ECM）、设备 List 中精确 `select_mbn` 名称，或独立 `reboot`。
不按 HPLMN 猜 MBN，不将关闭基带 IMS 当作用户态 IMS 的必需步骤。

- 计划 revision 绑定线路、controller 实例、动作与状态；apply 必须同一 action、
  `expected_revision`、`confirm_line_id`。门内重读，拒绝过期计划、活动 bearer、未知空闲通话或型号不符。
- 写前持久化 `session-<line>-maintenance.json` 及 owner/代次；HTTP 取消不打断物理事务。
  模式/MBN 写后回读；MBN 先显式关闭 AutoSel，再设置清单中的选项。
- `verified_setting` 只证明设置回读一致，不证明重启、重枚举、驻网或 IMS 成功。
  需要重启须另行明确请求，不自动 `CFUN`。
- `unconfirmed`/`reboot_requested` 保留 receipt、禁用旧控制代次，不逆向回写或自动重试。
  操作者须核实设备、映射和实际设置，不能通过删除 receipt 修复未知结果。
- 不写 USB VID/PID、APN、Initial EPS/NV，不提供任意 AT 入口。通用 native reset 不绕过维护，
  未实现的 SIM-only/其他 driver 返回 `native_reset_requires_explicit_maintenance_plan`。

## 10. DJI 一代 USB 模块维护

针对 VID/PID `2ca3:4006` 的 `simadmin dji-prepare --usb-device <sysfs-usb-node>` 默认只输出计划。
执行须另加 `--apply --confirm-usb-device <同一节点> --expected-generation <计划busnum:devnum>`；
不能复制另一台机器的拓扑/代次，不是开机自动修复器。

- Linux、精确 VID/PID、恰好接口 0–4、当前仅一台匹配模块；接口 4 必须尚未绑定。
- 操作者先停止 MM 及其他 AT/读卡/PPP 程序、加载 `qmi_wwan`/`option`；不存在 native owner 或
  未解决 receipt，工具持相同物理锁，不自动停服务、modprobe、改 USB 身份或拆活动数据面。
- 核验 USB 字符设备与代次后，向接口 4 发 CDC DTR、注册动态 ID、绑定 QMI 4 与串口 0–3，
  逐步回读，最后有界 `qmicli --dms-get-operating-mode`；不切 online，不把 DMS 可读说成 IMS 成功。
- `new_id` 是驱动级 VID/PID 规则，通常直到驱动卸载，非永久单端口规则；计划披露副作用，
  拒绝同型号多设备。仅纠正本次从未绑定状态新产生的串口误绑 QMI，既有/陌生状态人工处理。
- 中途失败持久化 `session-dji-<usb>-maintenance.json` 与已完成步骤；控制节点/代次变化即停。
  不自动回滚或重试，旧 `/run` 记录同样阻断。仅有代码/fixture 证据，无真实 DJI 写操作验收。

## 11. 能力扩展与验收门槛

MEP 仍是未来接口设计，**没有已交付的 MEP 能力或真实 MEP 验收**。普通 eSIM profile 管理、
独立 PC/SC 读卡器和 WiFi-only VoWiFi 不应受影响；读卡器只提供 APDU/AKA，不要求蜂窝联网。
MEP Port、UIM slot、reader index、物理槽与 `line_id` 必须分离。未来需独立 capability/Port/Profile
映射、SIM 来源及可插拔 backend；未知能力不发自定义 APDU，不靠临时切换 profile 伪造并发。
eUICC 级 APDU/lpac 仍互斥，Port/线路状态独立；MEP 不等于双蜂窝、双射频、双 IMS。

剩余实现与实机验收包括：混合 owner、未知资源的型号专属核对、AT-only PPP/ECM/NCM 数据面、
厂商 RAT/band/reset、多槽/MEP、基带自带 IMS 与用户态 IMS 争用，以及 Quectel/DJI 真实维护。
型号名、目录中的 driver、普通 5G 数据或模拟测试都不证明上述能力可用，也不证明 VoNR/CS 音频可用。

| 验收面 | 必须取得的独立证据 |
| --- | --- |
| 设备与协议 | 明确型号/固件/内核/接口组合；真实 QMI、MBIM、AT，SIM/AKA、PIN、拔插/eSIM 切换与代次变化 |
| 数据面 | IPv4/IPv6/双栈、网络强制单栈、地址就绪/DAD、P-CSCF、MTU、UE 移入/归还及故障清理 |
| IMS | 无 MM 环境的实际注册、AKA/安全协商；每声明组合至少两次原会话自然续期，不缩租期或重注册凑数 |
| 业务 | 短信/CNMI/CSMS/报告、来电/拨号/USSD、普通数据、代理与自动化分别验证；呼叫控制不等于双向音频 |
| 隔离与长稳 | 至少两条真实线路；故障互不影响；每目标硬件/固件代表场景连续 24 小时，记录泄漏与队列增长 |
| 恢复与工程 | 取消、强杀、掉电/重枚举、陈旧归属、磁盘满/只读、权限、升级/回滚和无 MM 包依赖审计 |

socket-pair、fake-sysfs、注入 IO、SQLite/PDU 与守卫测试只证明对应软件契约；缺硬件不记为通过。
N/A 必须有能力证据。真实电话/短信须另获授权；不自动拨号、发短信或拨紧急号码。
当前不做 SIM-04 native 接管，用户取消的测试窗口自动回滚也不能因本文示例恢复。

## 12. 相关手册

- [架构](ARCHITECTURE.md)、[设备驱动](DEVICE_DRIVERS.md)：跨后端身份、隔离与能力接口。
- [安装](INSTALL.md)、[开发者指南](DEVELOPER.md)：运维与工程入口；本文不授权执行写操作。
- [开发计划](DEVELOPMENT_PLAN.md)：跨项目优先级及 1.1.5/1.1.6 发布条件。
- [IMS 诊断](IMS_DIAGNOSTICS.md)、[410 基带故障](QCM410_BAM_DMUX_MODEM_CRASH.md)：分层故障边界。
- [历史档案](archive/README.md)：原始审计、参考项目许可证与证据；参考思路不等于允许复制其实现。
