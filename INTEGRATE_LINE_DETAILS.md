# SIM 线路工作台整合计划

> 更新日期：2026-08-14  
> 状态：本轮前后端实现并部署完成，等待目标设备 WebUI 验收  
> 范围：SIM 页面信息架构、线路状态、eSIM、自动化、通知与旧入口清理

## 1. 目标

把原来分散在“SIM 卡详情”弹窗、独立 eSIM 页面、独立读卡器区域、自动化中心和通知中心的线路能力，整合成统一的两栏线路工作台：

- 左栏统一列出基带卡槽和独立 SIM/eSIM 读卡器。
- 右栏只展示当前选中线路的信息与配置。
- 每条线路独立管理网络、VoLTE、VoWiFi、Trunk、运营商与小区信息、eSIM、自动化和通知；移动数据 APN 仅作为内部承载参数维护，不再提供独立编辑页。
- 删除重复入口和重复状态块，保留旧路由用于书签兼容。
- 窄屏时两栏自然变为上下布局，不产生第三栏拥挤或横向溢出。

## 2. 最终页面结构

### 顶层标签

SIM 页面保留两个顶层标签：

1. `线路与 SIM`
2. `运营商 Profile`

### 线路工作台

`线路与 SIM` 使用两栏布局：

- 左栏：设备列表，包含基带线路和读卡器线路。
- 右栏：线路摘要、接入进度和线路标签页。

右栏标签顺序：

1. 概览
2. eSIM
3. IMS 与 Trunk
4. 短信
5. 通知
6. 自动化

读卡器是一级线路。读卡器不依赖基带，因此仍提供概览、eSIM、IMS 中的 VoWiFi、短信、通知和自动化；基带专属的 VoLTE、蜂窝数据和 Trunk 控制不显示。

左侧设备栏在桌面端与右侧当前工作区等高，设备条目过多时只滚动设备栏内部；窄屏下设备栏限制最大高度并提供同样的纵向滚动，不让设备数量无限拉长整页。

## 3. 线路摘要与接入进度

摘要区显示：

- 设备/卡槽名称和运营商
- 短线路 ID、信号强度和驻网状态
- 当前语音接入类型及运行阶段
- 对应接入的分段进度条

### 接入选择顺序

同一时间只显示一个接入状态：

1. VoWiFi 已注册时显示 VoWiFi。
2. 否则，VoLTE 已注册时显示 VoLTE。
3. 没有已注册接入时，显示已启用且正在连接的 VoWiFi。
4. 否则显示已启用且正在连接的 VoLTE。
5. 都未启用时显示 CS。

读卡器没有 VoLTE/CS 基带能力，默认显示 VoWiFi 路径。

### VoWiFi：6 阶段

| 顺序 | 显示名称 | 运行时阶段 |
|---|---|---|
| 1 | SIM 身份 | `identity_ready`、`profile_matched`、`sim_auth_ready` |
| 2 | ePDG 接入 | `epdg_ready` |
| 3 | IKE 隧道 | `ike_ready`、`child_sa_ready` |
| 4 | ESP 通道 | `esp_ready` |
| 5 | IMS 注册 | `ims_registered`、`sms_ready` 或 `runtime_registered` |
| 6 | 语音就绪 | `voice_ready` |

### VoLTE：6 阶段

| 顺序 | 显示名称 | 运行时阶段 |
|---|---|---|
| 1 | SIM 身份 | `identity`、`identity_aka` |
| 2 | 无线接入 | `radio`、`modem` |
| 3 | IMS Bearer | `ims_context`、`bearer*`、`ip_config` |
| 4 | P-CSCF | `pcscf`、`ipv6_preflight` |
| 5 | IMS 注册 | `register_initial`、`ipsec`、`register_authenticated` |
| 6 | 注册就绪 | `registered`、`register_refresh`、`register_ipsec`、`register_udp` |

### CS：4 阶段

1. SIM 就绪
2. 已驻网
3. 数据已连接
4. 语音可用

当前后端没有独立的 CS 音频能力状态。本轮用 ModemManager 的 SIM、驻网和连接状态表示已有能力，不把 CS 音频链路宣称为已完成。后端补充明确的 CS voice capability 后，第四阶段应改为该权威字段。

## 4. 旧 SIM 详情拆分

删除 `LineDetailsDialog` 和“SIM 卡详情”按钮，不再维护第二套线路详情 UI。原弹窗内容按职责拆分：

| 原内容 | 新位置 |
|---|---|
| SIM 身份、号码、SMSC、锁状态 | 概览 |
| CS/基带状态、设备路径、主端口 | 概览 |
| VoLTE 运行态、P-CSCF、Bearer、连接尝试 | VoLTE |
| VoWiFi 阶段、Profile、代理与错误 | VoWiFi |
| Trunk 注册、端点、通话统计 | Trunk |
| 小区扫描与锁定 | 小区 |
| 移动数据 APN | 由线路内部数据承载逻辑使用，不提供独立 UI/API |
| 运营商选择 | 运营商 |

敏感字段继续默认遮罩，只能由用户主动显示。

### 概述布局

概述分为上下两部分：

- 上方使用两列、两行模块布局，中间不额外绘制分隔线。
- 左上为 `SIM 卡基本标识`。
- 右上为 `线路控制`，内部以四个独立边框项展示 `数据连接`、`漫游数据`、`飞行模式`、`重启基带`。
- 左下为 `设备、路径与存储`。
- 右下为 `安全与锁卡状态`。
- 下方整行使用大模块展示 `运营商与网络信息`，并容纳运营商、小区和网络详情。

### 离线配置与恢复

- 线路离线后仍可保存数据连接、代理、漫游、飞行模式、VoLTE IMS、VoWiFi 和 Trunk 配置。
- 离线保存只修改持久化意图，不调用已经缺席的 ModemManager 对象；设备重新出现时由线路恢复器应用配置。
- 离线基带仍显示可操作的 `重启此基带`。恢复时优先使用该线路保留的 QMI 控制口发送基带 reset，并只接受映射回同一 QMI 设备的 ModemManager 对象。
- 若 QMI reset 后仍未重新枚举，手动恢复最后才重启 ModemManager；确认框明确提示该回退可能短暂影响其他基带。
- 已保存飞行模式的线路在恢复结束后重新关闭移动射频，避免恢复流程覆盖用户意图。

### VoLTE 配置入口

- 已删除 VoLTE 的 `地址族`编辑按钮、弹窗和前端 API。运行时当前地址族仍作为只读诊断信息保留。
- 暂不直接把原按钮改名为笼统的`配置`，因为地址族编辑器与后续配置语义不一致。
- 待确认的首版候选项为自动恢复初始延迟、最大尝试次数、重试间隔、VoLTE 语音能力和 ViLTE 能力。
- P-CSCF、IMS 域名、鉴权和运营商能力字段继续归运营商 Profile 管理，不在线路弹窗重复维护。

## 5. eSIM 行为

每条线路保存三态控制：

- `auto`：`esim_control = null`，根据 `sim_type`/`esim_status` 探测 eUICC。
- `enabled`：`esim_control = true`，强制启用 eSIM 管理。
- `disabled`：`esim_control = false`，强制关闭 eSIM 管理和 lpac 调用。

eSIM 标签必须先请求 `GET /esim/lpac/status`：

1. lpac 可用时再读取 EID 和 Profiles。
2. lpac 不可用时不调用读卡命令，直接显示架构、glibc、目标安装包和安装位置。
3. 提供 GitHub 下载代理选择和“下载并安装 lpac”按钮。
4. 安装调用现有 `/esim/lpac/repair`，由后端选择兼容的旧版或当前版资产。
5. 安装成功后自动重新检测并加载 EID/Profiles。

lpac 配置分为两个作用域：

- 设备级 `EsimConfig` 只保存 lpac 二进制路径和芯片容量等安装参数。
- 线路级 `LineProfileConfig.esim_reader` 保存 APDU/HTTP 后端，以及 AT、QMI、PC/SC、MBIM 各后端的设备和槽位参数。
- eSIM 操作先用 `line_id` 读取线路 reader 配置，再解析该线路注册的读卡设备；禁止从全局配置选择某张卡。
- 旧版本的全局 reader 参数仅在发现一条线路时迁移一次；发现多条线路时不批量复制，避免多个 eUICC 指向同一端口。
- 继续通过配置存储中的 `app_config` 事务持久化 `LineProfileConfig`，不新增独立 reader 表；当前配置规模不需要额外关系表。

eSIM 标签标题区在“完整管理”左侧提供“lpac 接口”按钮。按钮打开与完整管理同宽、同标题栏结构的独立弹窗，包含：

- lpac 状态、兼容资产、GitHub 下载代理、第三方压缩包 URL 和安装/修复操作。
- 当前线路的 APDU/HTTP 后端选择。
- QMI 设备与 UIM 槽位覆盖、AT 端口、PC/SC reader 名称与接口索引、MBIM 设备/槽位/proxy/slot mapping 设置。
- 保存后重新读取当前线路 eSIM 数据；运行 lpac 时按官方变量名注入对应配置。

## 6. 线路自动化

复用现有 `AutomationTask.target`，不新增重复的 `line_id` 字段：

```text
task.target.kind = modem_line
task.target.line_id = 当前线路
```

线路标签行为：

- 只显示当前线路任务。
- 新建和编辑任务时固定目标线路，不能切换到其他线路。
- 线路模式不提供“重启整台设备”动作。
- 日志 API 和高级清理均强制带当前 `line_id`。
- 保留任务类型、执行状态、日期和关键字筛选。
- 保存时合并回完整 `AutomationConfig`，不能覆盖其他线路任务。

现有 JSON 配置足以支持当前任务规模。本轮不迁移数据库表。

## 7. 线路通知

复用 `NotificationRule.sim_channel_ids` 作为线路作用域，不新增 `scope_line_ids`：

- 空数组：设备级/全部线路规则。
- 基带规则包含当前稳定 `line_id`；读卡器规则包含后端通道 ID `reader:<slot_id>`。

线路标签行为：

- 只编辑 `sim_channel_ids` 包含当前线路的规则。
- 新建规则自动写入当前线路的通知作用域 ID；读卡器使用 `reader:<slot_id>`，基带使用稳定 `line_id`。
- 线路规则提供短信、通话和自动化事件类型，这三类后端事件具有 `line_id`。
- 日志和失败队列固定按当前线路查询，并在前端再次过滤队列。
- 批量重试/删除只操作当前线路队列项。
- 通知通道和日志自动清理策略仍是设备级共享配置。
- 设备级全局规则继续生效，但不在某条线路的规则列表中修改。
- 保存时合并回完整 `NotificationConfig`，不能覆盖其他线路或全局规则。

当前后端 `rule_matches()` 对 `SystemEvent` 有专门分支，且 DDNS、版本更新、设备状态没有线路 ID，因此这些事件不能伪装成线路专属规则。若后续需要线路级系统事件，先扩展后端事件契约再开放 UI。

## 8. 导航清理

- 从侧边栏删除“自动化与通知”整个分类。
- 删除基本配置页的“SIM / eSIM 管理”卡片，保留基本配置路由和空页面容器供后续使用。
- 暂时保留 `/automation` 和 `/notifications` 路由，兼容旧书签和设备级全局配置。
- 删除页面对独立读卡器面板的引用；设备列表统一承载基带与读卡器。

## 9. 数据结构结论

本轮不拆分新的自动化/通知关系表，继续使用现有版本化配置结构：

- 自动化已有 `task.target.line_id`。
- 通知已有 `rule.sim_channel_ids`。
- 自动化日志、通知日志和通知队列 API 已支持 `line_id`。
- 配置规模较小时，版本化 JSON 比拆表更容易保持向后兼容。

基带线路身份已改为物理槽作用域：

- `line_id = hash(物理槽锚点 + UIM slot)`，同槽更换 ICCID 后保持不变。
- 旧版 `hash(物理槽锚点 + ICCID)` 加入 `legacy_line_ids`，首次发现时复制线路配置并重写自动化目标、通知 `sim_channel_ids`。
- `line_data_traffic` 将旧 ID 的累计值事务合并到新 ID，并删除旧流量行，重复刷新不会二次累加。
- 短信、通话和诊断历史保留写入当时的原始 ID，作为历史审计事实；不批量伪造其原始线路归属。
- SIM IMS 覆写继续使用 `SimBindingKey`（普通卡 ICCID，eSIM 为 EID + profile ICCID），不使用物理 `line_id`。

只有满足以下任一条件时再考虑迁移 `automation_tasks`、`notification_rules` 关系表：

- 单设备达到数百条任务/规则。
- 需要多客户端并发局部更新。
- 需要按线路做 SQL 聚合、外键和事务更新。

迁移时必须为线路字段建立索引，并保留旧 JSON 的一次性导入和向后兼容读取。

## 10. 实施状态

- [x] 两栏线路工作台和统一设备列表
- [x] eSIM 三态控制
- [x] 动态 VoWiFi/VoLTE/CS 进度条
- [x] 原 SIM 详情内容拆分到子标签
- [x] 删除 `LineDetailsDialog` 和详情按钮
- [x] 自动化中心线路嵌入模式
- [x] 通知中心线路嵌入模式
- [x] lpac 缺失时的安装/修复入口
- [x] lpac 工具与线路接口拆为 eSIM 管理内的独立弹窗
- [x] lpac APDU/HTTP/AT/QMI/PCSC/MBIM 参数按线路保存并提供线路 API
- [x] lpac QMI、PC/SC、MBIM 环境变量映射和按线路重载持久化测试
- [x] 基带线路 ID 改为稳定物理槽 + UIM slot，并迁移旧线路配置引用与累计流量
- [x] 删除侧边栏自动化与通知分类
- [x] 删除基本配置页 SIM/eSIM 卡片
- [x] 删除仪表盘快捷控制和顶部基带重启入口
- [x] 删除 APN 独立标签、编辑 API 和公开路由，保留内部数据承载 APN 参数
- [x] 扩展运营商 Profile 编辑字段并删除 E911 编辑 UI
- [x] 概述改为上方四模块、下方整行运营商与网络信息
- [x] 删除 VoLTE 地址族编辑 UI 和前端写入 API
- [x] 左侧设备栏与工作区等高并在设备过多时内部滚动
- [x] 离线线路可保存数据、漫游、飞行模式和 IMS/Trunk 配置
- [x] 离线基带按保留 QMI 控制口执行定向恢复
- [ ] 确认并实现新的 VoLTE `配置`弹窗范围
- [x] TypeScript 类型检查
- [x] ESLint
- [x] Vite 生产构建
- [ ] 桌面和窄屏 WebUI 人工检查
- [x] 将本轮增量重新部署到 `192.168.100.13:3300`（2026-08-14，服务与健康检查通过）
- [x] 收紧概述线路控制高度，并将安全与锁卡状态固定在第二行右侧
- [x] OTA、运营商 Profile 与 lpac 共用可自定义的 GitHub 下载加速设置
- [x] OTA Release 源切换到 `autisticryptic/SimMaster`（当前仓库尚未发布 Release）
- [x] 顶部新增 ModemManager.service 重启入口
- [x] 设备网络并入基本配置并清理侧边栏网络与仓库入口
- [ ] 410 设备实机验收（需用户在浏览器中完成线路切换、eSIM、自动化和通知操作验收）

## 11. 主要改动文件

| 文件 | 职责 |
|---|---|
| `frontend/src/pages/SimCard.tsx` | 顶层页面、线路摘要、进度条、eSIM、嵌入中心 |
| `frontend/src/pages/sim/ModemLinesPanel.tsx` | 两栏设备工作台和线路标签 |
| `frontend/src/pages/sim/LineRuntimeDetails.tsx` | CS、VoLTE、VoWiFi、Trunk 运行详情 |
| `frontend/src/pages/sim/LineCellularSettings.tsx` | 概览中的运营商与小区信息、小区锁定工具 |
| `frontend/src/pages/AutomationCenter.tsx` | 自动化线路作用域 |
| `frontend/src/pages/automation/AutomationTaskDialog.tsx` | 固定线路任务编辑 |
| `frontend/src/pages/NotificationCenter.tsx` | 通知线路作用域和完整配置合并 |
| `frontend/src/pages/notifications/*` | 通知日志、规则和通道嵌入布局 |
| `frontend/src/components/Layout/Sidebar.tsx` | 删除旧导航分类 |
| `frontend/src/pages/Configuration.tsx` | 删除旧 SIM/eSIM 配置卡片 |
| `frontend/src/pages/sim/LineDetailsDialog.tsx` | 已删除 |

## 12. 验收清单

### 页面布局

- 桌面宽度下左栏固定为设备选择，右栏不换到第三栏。
- 窄屏下左右两栏变为上下布局，标签可横向滚动。
- 长线路 ID、运营商名、设备名和错误信息不覆盖按钮或相邻内容。

### 线路隔离

- 切换设备时摘要、标签内容、eSIM、任务、规则、日志和队列同步切换。
- 编辑一条线路不会改变另一条线路的任务和规则。
- 读卡器能进入 VoWiFi/eSIM/自动化/通知，不显示基带专属标签。

### eSIM

- `auto/enabled/disabled` 刷新页面后保持一致。
- lpac 缺失时不被通用读卡错误遮挡，并可完成下载/安装。
- 安装失败展示后端错误，安装成功后自动读取 Profiles。

### 构建与部署

```bash
cd frontend
pnpm lint
pnpm type-check
pnpm exec vite build
```

部署前备份目标机 `/root/simadmin-codex/www`，再原子替换静态资源。部署后检查：

```text
http://192.168.100.13:3300
```

本轮包含后端线路身份与兼容迁移逻辑。部署时必须同时替换后端二进制；不覆盖 `/data/config.sqlite3`、`data.db` 或运营商 catalog，并在启动前分别备份现有二进制、Web 静态资源和数据库文件。
