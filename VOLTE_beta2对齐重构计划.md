# VoLTE 对齐 beta2 重构计划

> 目标：以 `SimAdmin-VoLTE/simadmin beta2`（1.1.7-beta2）逆向为准，重构本项目 VoLTE
> 对底层 ModemManager / QMI 的操作，使注册前的 bearer / P-CSCF / 数据槽位分配贴合 beta2。
> 方法：IDA 提取 beta2 行为差异 → 补进现有代码（现有代码本身已是 beta2 的 clean-room 克隆，
> 架构层已 ~90% 对齐，只差几处**行为顺序**与**缺失的分层**）。

---

## 进度（滚动更新）

- ✅ **阶段 1（P-CSCF 顺序）已完成**：把会崩基带的 AT `$QCPDPIMSCFGE`+`CGACT` 探测
  从"每次连接前无条件跑"改为 beta2 顺序——bearer 先起 → 用 bearer 自带 P-CSCF
  （MM 的 PCO / native 的 WDS 直读）→ 只有拿不到时才最后跑 AT 兜底。移除了不再需要的
  prefix-unavailable workaround。150 个 volte 单测全绿。
- ✅ **阶段 2（data slot mode）已完成**：新增 `access/volte/data_slot.rs`，实现
  beta2 三态 `DataSlotMode`（`independent_wwan1` / `secondary_qmi_data` /
  `both_data_slots_active`）+ `select_data_slot_mode` 选择逻辑（含
  `volte_data_slot_mode_missing` / `volte_data_slot_conflict` 冲突态）。runtime 的
  `data_path_mode` 从展示串 `dedicated_ims_bearer_N` 升级为由 enum 派生的 beta2 token。
- ⏳ 阶段 3（native 接入，保持 env 门控）、阶段 4（就绪门 + 健康检查）待做。

---

## 一、IDA 已确认的 beta2 行为（关键锚点）

- **data slot mode**（`sub_58E0C4`，volte.rs:1676-1687）：一个返回 0/1/2 三态的选择函数，
  对应 `IMS allocated to primary qmi0; DATA6 is reserved for data` /
  `IMS allocated to DATA6; primary qmi0 is reserved for data` /
  `volte_data_slot_mode_missing`，并有 `both_data_slots_active` / `independent_wwan1` /
  `secondary_qmi_data` / `volte_data_slot_conflict` 状态。**能跑通的是"IMS 在主口、DATA6 让给数据"。**
- **P-CSCF 四级发现，bearer 先起再读**（顺序按 volte.rs 行号）：
  1. profile 预取（1511 `prefetched from IMS profile` / 2162 `volte_runtime_profile_pcscf_missing`）
  2. bearer 起来（1590 `IMS bearer is up`）
  3. **直读 QMI WDS**（2022-2054 `discovered directly from QMI WDS` / `keeping AT fallback` / `CID is not numeric`）
  4. **AT CGCONTRDP 兜底**（2192 `discovered from active IMS bearer`）
- QMI provisioning 就绪门：`/run/qmi_auto_activate.ready`（`Waiting for initial QMI UIM provisioning to settle`）。
- 健康检查含 secondary QMI packet-status（`Secondary QMI packet status was inconclusive`）。

## 二、与本项目的差异（已逐条对照代码确认）

| # | 维度 | 本项目现状 | beta2 | 风险 |
|---|---|---|---|---|
| A | **P-CSCF 顺序** | **AT 探测在最前**（`live.rs:615`，bearer 之前跑 `$QCPDPIMSCFGE`+`CGACT`，即崩基带那条），bearer PCO 兜底 | bearer 先起 → WDS 直读 → AT **最后**兜底 | AT 前置=**主动触发崩基带**，这是当前最危险的点 |
| B | **native QMI 路径** | env 门控 `SIMADMIN_VOLTE_NATIVE_IMS_BEARER`，默认走 MM | 主路径（由 data slot mode 选择） | 默认翻转会触发未验证的主口激活 |
| C | **data slot mode** | 只有展示字符串 `dedicated_ims_bearer_4`，无真实三态选择/冲突检测 | 完整三态 + 冲突检测 | 纯逻辑，可测 |
| D | **profile P-CSCF 预取** | VoLTE 无（仅 VoWiFi 有） | 有，作为第一级 | 纯逻辑，可测 |
| E | **QMI 就绪门** | 无 | `/run/qmi_auto_activate.ready` | 低 |

## 三、实施计划（按价值/风险排序，分阶段）

### 阶段 1 — P-CSCF 发现顺序对齐 beta2（差异 A + D，最高价值、可全测、**降低**崩基带风险）
`pcscf.rs` + `live.rs::connect_inner`：
1. 新增 `discover_pcscf`（分层）：**先** bearer/WDS current-settings（`settings.pcscf`，解析已存在）
   → **再** 直读 QMI WDS CID（复用 `qmi_wds::current_settings`）→ **最后**才 `discover_pcscf_via_at_with_context`。
2. 把 `live.rs:615` 的 AT 前置探测**降级为兜底**：仅当 bearer/WDS 都没给出 P-CSCF 时才跑 AT。
   保留现有 `ImsAtContextLease`（prefix-unavailable workaround），但只在真正走 AT 时启用。
3. 新增 profile 预取层（差异 D）：若线路存有 IMS profile 的 P-CSCF，先用它，失败记
   `volte_runtime_profile_pcscf_missing` 再降级。
4. 新增错误码 / 日志字符串对齐 beta2（`Native VoLTE P-CSCF candidates discovered directly from QMI WDS` 等）。
- **可验证**：全部单测（顺序、降级、family 过滤），无需真机。

### 阶段 2 — data slot mode 数据模型 + 选择逻辑（差异 C，纯逻辑、可测）
新增 `access/volte/data_slot.rs`：
- `enum DataSlotMode { PrimaryImsSecondaryData, SecondaryImsPrimaryData, BothActive, ... }`
  对应 beta2 三态 + 冲突。
- `select_data_slot_mode(config, capabilities) -> Result<DataSlotMode, VolteError>`，
  照 `sub_58E0C4` 的分支（data_requested → 能力检查 → 0/1/2），冲突返回 `volte_data_slot_conflict`。
- runtime 的 `data_path_mode` 从"展示串"升级为由本 enum 派生的真实状态。
- **可验证**：单测覆盖三态 + 冲突 + 缺失。

### 阶段 3 — native 路径接入 data slot（差异 B，**触及真机崩基带风险**）

**已拍板：保持 env 门控为默认，beta2 完整做法写入本文档备用。**

- 让 data slot mode = "IMS 主口" 的**接线逻辑就位**，但入口仍由 `SIMADMIN_VOLTE_NATIVE_IMS_BEARER`
  门控——默认路径**仍走 MM**。原因：主口 `--wds-start-network=apn=ims` 激活在参考基带上从未真机
  验证，坏激活可能 SSR 甚至整机重启。
- 保留 `FailureClass::BasebandWedged` 立即中止（已有），wedge 不回落 MM（避免二次激活）。
- data slot mode 选出的"IMS 主口/DATA6 数据"结果，会喂给 native 路径的接线，但只有 env 打开时才实际执行；
  env 关闭时 mode 仅作为 runtime 展示与后续决策依据，不改变默认 MM 行为。

#### beta2 完整做法（后期若默认 MM 仍无法激活/仍崩基带，则完全照抄此流程翻默认）

> 依据 IDA 锚点 + `VOLTE_真机实测结论_QMI端点能力.md`。这是"完全照抄 beta2"的落地清单，
> 触发条件：默认 MM 路径在真机上仍 `Ipv6OnlyAllowed`→崩基带，或拿不到 P-CSCF。

1. **端点选择**：IMS bearer 跑主口 `/dev/wwan0qmi0` + `--device-open-proxy`（qmi-proxy 在
   `/usr/libexec/qmi-proxy`，不在 PATH）。DATA6/DATA7（`wwan0qmi1/2`，自编译内核模块）**只给数据**，
   绝不给 IMS 跑多步流程（DATA6 无法跨进程复用 CID）。
2. **CID 分配与复用**：`--wds-noop --client-no-release-cid` 分配 CID → 同 CID 复用跑
   `--wds-set-ip-family` → `--wds-start-network=apn=ims,3gpp-profile=N,ip-type=4`（**先 IPv4**，
   Maxis 卡 ip-type=6 被网络拒 `[3gpp] ipv4-only-allowed`）→ `--wds-get-current-settings` 取 P-CSCF。
3. **硬约束（少一个就崩）**：
   - 绝不用任何 `--wds-bind-data-port` / `--wds-bind-mux-data-port`（2015 固件不支持，直接 SSR）。
   - 辅助端点打开必须带 `--device-open-net='net-raw-ip|net-no-qos-header'`。
   - 主口多步流程必须 `--device-open-proxy` 且 qmi-proxy 在跑。
   - qmicli 一次只允许一个 WDS 动作。
4. **netdev 解析**：bam-dmux 无法显式绑 mux，起会话后逐个 `wwanN` 发探测流量按 rx 计数判定承载 netdev，
   探不到回落 `Assumed`（标记未验证）。
5. **wedge 识别**：`endpoint hangup` / `interface-in-use-config-match` / `MobileEquipment.Unknown`
   →`FailureClass::BasebandWedged`→立即中止整个重试批次，**不回落 MM**（二次激活会把 SSR 升级成整机重启）。
6. **翻默认的动作**：把 `native_ims_bearer_enabled()` 默认值改 true（或由 data slot mode 直接驱动），
   env 变为"强制关闭"开关。翻默认前必须在真机验证 `--wds-start-network=apn=ims,ip-type=4` 主口激活
   不崩基带（memory 记录：前面每步都验过，唯独这一步没验）。
7. **DATA6 运行时依赖**：`secondary-qmi-init` 在 MM 启动前 hold 住 DATA6 端点；udev 给**所有**空闲 QMI 口
   写 `ID_MM_PORT_IGNORE=1`（含 DATA7 缺口）；watchdog 在主 QMI 重启后重建 DATA6。

### 阶段 4 — 就绪门 + 健康检查（差异 E，低风险收尾）
- **已实现**：新增 `access/volte/readiness.rs`，`connect_inner` 起流程前等
  `/run/qmi_auto_activate.ready`（超时则继续，照 beta2 `continuing with modem readiness checks`）。
  纯逻辑 + 注入时钟/文件谓词，5 个单测覆盖"立即就绪 / 轮询后就绪 / 超时兜底 / 零间隔不空转"。
- **secondary QMI packet-status 健康检查——记入文档备用，本轮不实现**：
  - 现有健康机制是 `live_receive_loop` 的 REGISTER 刷新周期（`refresh_live_registration`
    失败→转 Degraded→`cleanup_live_session` 重建），无独立 bearer 探测函数。
  - beta2 的 `Secondary QMI packet status was inconclusive; retaining live host IMS state` /
    `volte_runtime_health_qmi_disconnected` 判据只对 native/DATA6 数据槽有意义，而 native 默认
    env 门控关闭。为它加一层会碰 QMI 口的探测循环，只在 native 打开时有价值，且增加基带风险面。
  - **beta2 做法（后期翻默认 native 时一并实现）**：session 持有 secondary QMI CID，健康周期里发
    `--wds-get-packet-service-status`；`disconnected`→`volte_runtime_health_qmi_disconnected` 转
    Degraded；查询失败/不确定→`retaining live host IMS state`（不误杀，保留当前状态）。与阶段 3
    翻默认绑定，同批落地。

## 四、已定的决策

- **做全部 4 阶段。**
- **阶段 3 保持 env 门控为默认**（默认仍走 MM）；beta2 完整做法已写入阶段 3 的"beta2 完整做法"小节，
  作为后期若默认 MM 仍无法激活 VoLTE / 仍崩基带时"完全照抄 beta2 翻默认"的兜底清单。
- 阶段 1/2/4 无崩基带风险，直接落地。

## 五、验证

- 每阶段结束跑 `cargo test`（现有 682 测试基线）+ 新增单测。
- 阶段 1/2/4 纯代码，编译+测试即可交付。
- 阶段 3 若翻默认，需真机（192.168.100.13，Maxis 50212）验证主口 IMS 激活不崩基带。
