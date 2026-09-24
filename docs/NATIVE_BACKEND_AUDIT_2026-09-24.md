# 原生硬件接口审计与增强计划（2026-09-24）

> 范围：检查 1.1.4 → 1.1.5 “ModemManager 可选 / 本项目直接操作硬件接口”的完成情况，
> 并对照用户指定的 5 个参考项目，给出能增强直接硬件接口完备性的具体条目。
> 审计基线为原 `dev/1.1.5-modem-backends` 的 `71513ea`；本轮成果已整合到 `master`。
> 分支收口过程见 [分支整理记录](BRANCH_CONSOLIDATION_2026-09-24.md)。
> 每项按“代码 / CI / 实机”分别记录，不因编译通过而勾选实机。

## 1. 完成情况结论

**默认 MM 与显式 native 选择已接线，但 native 端到端实机验收仍为零；本轮补充只读发现与 AT/URC 基础能力。** 具体：

| 方面 | 状态 | 依据 |
|---|---|---|
| 后端选择 | 已完成（代码+CI） | `backends/config.rs`：默认 MM；`mode: native` 需 `allow_unvalidated_native: true`；未知目标不回退 MM |
| 协议控制器 | 已完成（代码+CI） | QMI DMS/NAS/UIM/WDS、MBIM、AT；按物理设备串行、flock、超时与输出上限（`native.rs`、`io.rs`、`bearer.rs`） |
| SIM/AKA、承载、UE 数据面 | 已完成（代码+CI） | QMI UIM / AT CCHO-CGLA；QMI/MBIM 会话、receipt、namespace 归还确认 |
| 短信/电话/USSD | 部分 | AT 命令与 URC 分流、原生短信事件提示触发扫描＋15 秒兜底已补；电话仍主要 `CLCC` 轮询，尚非完整事件驱动业务层 |
| 设备发现 | 代码/CI及SIM-04只读运行通过 | `discover-native` 只读扫描 sysfs，输出端口/物理锚点建议与不完整配置；不自动启用 native，不猜 IMS/data 映射 |
| Quectel | 专用驱动仅分类 | `devices/quectel/` 主要提供型号分类；native 已可走通用 AT/QMI 控制，但没有 Quectel MBN/USB composition 专用管理 |
| 混合 owner | 未实现 | 同机 MM/native 分设备并行未接通；当前全局二选一 |
| 代次恢复 | 未实现 | 控制节点代次变化需重启；无自动孤儿会话 reconciliation |
| 实机验收 | **无** | 已实测的 IMS 注册/续期（SIM-03、SIM-04 T03/T04/T05、`71513ea`）全部走 **MM 路径**，不能算 native 证据 |
| 路线图 | M0–M5 全部未勾 | `MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md` §5.2/§5.3 |

因此可以说“**代码提供显式实验性 native 选项**”，不能说“无 MM 全能力已经验收”。
只读发现降低配置调查成本，但不能证明候选端口可用、不会与已有 owner 冲突或支持 IMS。

## 2. 参考项目与可用方式

SimAdmin 为 GPLv3。以下判断决定“能拿代码”还是“只能借鉴思路（净室重写）”：

| 项目 | 语言 | 许可证 | 可用方式 | 与本项目最相关的内容 |
|---|---|---|---|---|
| [VoCat](https://github.com/MengMengCode/VoCat) | Go | 自定义研究/评估许可（禁商用） | **仅思路** | 无 MM 的 Quectel EC20/EC25 面板：sysfs 驱动绑定发现、端口角色、`AT+QCFG="ims"`、`AT+QMBNCFG` MBN 选择、DJI `2ca3:4006` 驱动绑定修复、原生 QMI NAS/飞行模式、QMI 端口租约 |
| [mdd-sim-gateway](https://github.com/MddIdd/mdd-sim-gateway) | Python | GPLv3 | 代码许可兼容；但其架构依赖 MM，价值在思路 | 模块 SIM 的逻辑通道容量/分配/失败释放；DJI/EC25 经 AT 建虚拟读卡通道 |
| [DJIModeSwitcher](https://github.com/hiwangchuan/DJIModeSwitcher) | Swift (macOS) | Apache-2.0 | 可引用（署名） | DJI 一代 4G 模块：批量端点上 `AT`/`OK` 探测 AT 接口；`AT+QCFG="usbnet",0/1` 切换并重启、等待重枚举、回读验证 |
| [EC25Toolbox](https://github.com/skyrocketingHong/EC25Toolbox) | Swift (macOS) | AGPLv3 | **本轮仅借鉴思路**（不引入额外许可证义务） | 单读者 AT transport：行分帧、`>` 提示、终结码收集、URC 分类与可重连事件分发；`QCFG usbcfg` 身份切换带备份/重启/验证/回滚 |
| [DJOneHub](https://github.com/ZenGeekLabs/DJOneHub) | Go (macOS) | PolyForm Noncommercial | **仅思路** | `apduarbiter`：多方（eSIM/短信/读卡）APDU 访问仲裁；换卡/拔插处理 |

macOS/libusb transport、模块 PCM 语音（`AT+QPCMV`）、MaVo/ADB 注入与本项目的 Linux 服务
形态无关，不纳入。

## 3. 增强条目（按收益/风险排序）

### N1 只读原生设备发现 `simadmin discover-native` — 最高优先

原因：手写设备表是启用 native 的主要障碍；本命令只读文件系统，不打开设备节点或改变 owner。
实现与使用说明见 [原生设备只读发现](NATIVE_MODEM_DISCOVERY.md)。

- [x] USB 按 `qmi_wwan` / `cdc_mbim` / `option` / `qcserial` 驱动发现；
      `cdc_acm` 只在同设备另有 modem 证据或已知厂商身份时纳入，避免把 Arduino 当基带
- [x] Quectel `2c7c` 的 ECM/RNDIS/未绑定组合，以及 DJI `2ca3:4006` 保留诊断条目
- [x] WWAN 严格解析 qmi/mbim/at 端口；MHI 兄弟通道归并到 PCI function，USB/WWAN 视图去重
- [x] Quectel 惯例端口只作 hint；多 AT 口不选第一个，候选 `at_device` 留空等待独占窗口探测
- [x] USB sysfs 锚点及 `busnum:devnum` 观察；硬件键/slot **必须人工核对**，不宣称切换后
      line ID 必然不变。SIM-04 的 MM `Device=qcom-soc` 与 sysfs key 就是不同的
- [x] 缺 QMI/MBIM 控制口、未绑定驱动、AT 缺失/未探测、多控制口/协议显式报告；
      多控制口不生成任选第一项的配置
- [x] JSON 输出可审阅的 `candidate`，只在协议/控制口唯一时生成；`ims`、`data`、`at_device`
      留空，`sms_reception_enabled=false`，不打开端点，不写配置
- [x] 12 项 fake-sysfs Rust 回归接入两套 CI；4 项 Python 被动边界守卫
- [x] CI 与两架构构建通过：`7a15a7f`，Validate `35946394878` / Build `35946394856`，
      发布 skipped；本轮收尾 `2129282` 的两套 CI 也通过
- [x] 实机只读发现验证：`2129282` 在 SIM-04 的 MM 正式服务运行期间成功；
      1 个物理 modem / 1 个 QMI / 2 个 AT、7 个宿主可见网口，AT 歧义和键/slot 待确认明确报告；
      应用 PID/MM/管理路由未变化。**不等于 native owner 接管或注册验收**，详见
      [9/24 续接记录](SIM04_CONTINUATION_2026-09-24.md)

### N2 URC 感知的 AT 会话（分流与短信调度已实现，完整业务事件层仍待补）

本轮在既有 `at_session.rs` 单读者互斥会话上增强，净室实现标准 AT 分帧/归属，
不复制参考项目的 transport 代码。使用与限制见 [原生 AT 事件处理](NATIVE_AT_EVENTS.md)。

- [x] 保持同一 AT 口单读者；命令响应与已知 URC 分离，查询同名前缀保留给查询；
      `NO CARRIER`/`BUSY` 等只终止拨号/接听事务，不误伤信号/SIM/短信查询
- [x] `>` 仅在帧起始识别为短信提示，不把 USSD 文本中的 `>` 当提示；
      输入帧、总响应及排空读取均设上限
- [x] 分类 `+CMTI`/`+CMT`/`+CDS`/`+CDSI`、来电、USSD、注册和 `+QIND`；
      只保留固定大小、无号码/正文的合并事件提示，不广播敏感原文
- [x] 原生短信在准入后每秒被动读取提示，通过同一物理门及端口代次前后核验；
      事件触发既有持久化接收/索引内容校验流程，15 秒完整扫描保留作丢事件兜底
- [x] 命令/提示交错、分片、URC 排空保留、SMS 提示与 reference、帧上限等离线测试已编写，
      两套 CI 新增实际执行过滤器；本地仅格式与 Python 守卫
- [x] 该轮新 Rust 测试与构建 CI 通过：`efe6135`；Validate `35951689917` / Build
      `35951689850` success，arm64/amd64 success，Publish Release skipped
- [x] 通话/注册事件有界订阅与独立广播，唤醒 CLCC/线路权威核对，落后订阅者全量核对；
      native 空闲通话/线路采用低频兜底，活动通话保留结束判定；API事件仅含提示
- [ ] 新业务事件接线 CI；跨代次断口/孤儿资源自动恢复仍不启用（同代次传输错误可重开，
      控制节点换代仍要求重新核对，不能盲目复用 CID）
- [ ] 直接 `+CMT` 正文与 `+CDS` delivery-report 的完整业务消费（当前仅隔离并触发核对）；
      配置仍使用存储通知，不因收到一个 hint 就声称短信已入库或发送成功
- [ ] native 真机短信/电话长稳验收

### N3 Quectel 设备驱动（EC20/EC25/EG25）

先只读诊断，再做受确认保护的写入：

- [x] 只读诊断实现：CGMM 型号核验、QCFG ims/usbnet/usbcfg、QMBNCFG List/AutoSel；
      未确认字段与不支持型号不猜测，见 [专项维护](NATIVE_DEVICE_MAINTENANCE.md)
- [x] 受确认写入实现：精确线路＋状态 revision 的 plan/apply，空闲/资源检查、写前 receipt、
      IMS/USB 模式与显式 MBN 选择（先关闭 AutoSel）写后回读；独立显式 reboot，
      不自动按 HPLMN 选 MBN、不自动重启或逆向回写；不确定结果 fence IO 并保留 receipt
- [x] 专项维护 CI：`ed508af`，Validate `35960810685` / Build `35960810703` success；
      首轮未声明 sha2 的编译问题已改用既有 ring 修正
- [ ] 真实 Quectel 固件验收（不能由注入式 IO 测试替代）
- [ ] 设计问题待实机确认：用户态 IMS 注册时，基带自带 IMS 客户端是否会与本项目争用同一
      IMS PDN/注册（EC20 CEFA 固件差异正是这一类问题）。候选做法是由本项目注册时
      强制关闭基带 IMS（`QCFG ims=2`），但必须在实机上先验证不影响承载与 P-CSCF 下发

### N4 DJI 一代 4G 模块（`2ca3:4006`）驱动绑定修复

- [x] 显式 `dji-prepare` CLI 已实现：默认被动计划；确认 exact USB port + 代次、单设备、
      owner/receipt、未绑定 QMI 接口和驱动前置后，DTR、接口 4 QMI、0–3 串口绑定及 DMS 只读检查；
      动态 ID 驱动级副作用明确披露，不写 NV/USB 身份，不自动停 MM 或回滚
- [x] 识别为 EC2x 的 DJI 模块可使用 N3 的显式 usbnet 设置与独立重启入口，
      重枚举后重新检查发现结果/端点，不将旧代次复用当成恢复
- [x] DJI 代码 CI：`32df051` 修复 musl/glibc ioctl 参数 ABI 后，Validate `35968312013` / Build
      `35968312015` success，amd64-musl/arm64-musl成功，发布 skipped
- [ ] 实际 DJI 驱动绑定/DTR/重枚举验收（未在SIM-04执行）

### N5 SIM 逻辑通道与 APDU 仲裁

- [x] AT/QMI 通道用途、slot、已确认 client/channel、open/close 计数及未知容量可见；
      写前 receipt、确认关闭才释放、未知结果保留，新增 [SIM 通道账本](NATIVE_SIM_CHANNEL_LEDGER.md)
- [x] eSIM(lpac) 与 IMS AKA 共用物理门，增加外部操作范围；不虚构 lpac 内部通道 ID
- [x] 账本新增回归 CI：`16ee44e`，Validate `35963157944` / Build `35963157995` success；
      QMI UIM 基础 codec 回归也补入两套实际执行过滤器
- [ ] native 真机通道故障/容量验收（不能用已通过的 MM IMS 验收替代）

## 4. 边界

- VoCat / EC25Toolbox / DJOneHub 的代码不复制，只按思路净室实现；引用 DJIModeSwitcher
  （Apache-2.0）时保留署名。
- SIM-04 继续作为 **MM 路径** IMS 测试机；native 实测需另一台设备（用户计划采购的 EC20 类设备）。
- N1 只读；N3/N4 的写操作必须有显式确认、回读与失败报告，不做静默兜底。
- 实机未验证前，文档与 UI 不得宣称“MM 可选已完成”。

## 5. 变更记录

| 条目 | 提交 | 说明 |
|---|---|---|
| — | `71513ea` | 审计基线；该版本 SIM-04 MM 路径已实机注册及自然续期 |
| N1 | `7a15a7f` | 被动发现、12 项 Rust 回归、4 项 Python 守卫；双架构 CI 通过，无硬件写入 |
| N2 基础 | `efe6135` | AT/URC 分流、有界读取、经准入的短信事件提示调度；双架构 CI 通过，未做 native 实机业务测试 |

参考仓库审计快照：VoCat `484cd23`、mdd-sim-gateway `8d9a830`、DJIModeSwitcher
`6d86b64`、EC25Toolbox `12678de`、DJOneHub `f7f1a0d`。只读发现是本项目自行实现，
未复制这些项目的硬件控制实现。N2 的分流/短信提示调度在本轮继续补强；
N2 剩余业务层与 N3–N5 不能因这些基础工作完成而一并勾选。
