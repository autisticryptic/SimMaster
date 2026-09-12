# SimAdmin 项目进度与续接交接文档

> 整理日期：2026-09-12，时间均为 Asia/Shanghai，另有注明除外。
> 本轮于 2026-09-12 上午重新读取两份完整会话的结构化记录，并核对已有交接草稿。
> 原会话最后有效设备快照约为 00:40，最新 CI/下载结果为 00:46:40；中止不等于候选已部署。
> 最初整理阶段仅核对本地源码、Git 和产物。用户随后授权继续实机工作，08:25 起的新进展见第 4.3 节。
> 本文可独立用于新对话。所有设备状态都有采样时间，不代表阅读本文时仍然如此。

**快速阅读：**第 1–3 节看结论和硬约束，第 7–9 节直接接续操作，第 11 节可复制到新对话。后续多卡测试明细统一追加到第 12 节，始终放在文末固定尾注之前。
用户已要求将本文、版本规划及文档索引一并提交 GitHub；异地续接应确认克隆的是包含这些文件的修复分支，不能只取旧 master。`.codex-*` 辅助脚本和原始会话不随文档提交。本文是交接快照，不代替全项目长期开发计划。

## 1. 当前结论

**最新结论（21:04–21:10 验收）：SIM-03 在 `684e2a7` 上的完整认证、初始注册及首次原会话自然续期已通过。19:14:13 初始注册，21:04:14 使用原 UDP socket 发送 CSeq=4 并成功续期，网络重新给出 7200 秒租期，refresh_count 从 0 增到 1，reconnect_count 保持 1。48 次只读样本、journal、程序哈希、进程、MM Bearer/3、归属账本、UE namespace 和 socket inode 均已交叉核对；不是重建后注册。观察器已完成退出，设备连接仍保留。见第 4.11 节。旧卡/IPv6 与 Pixel 呼入问题仍未验收，未合并 master、未升/发布 beta4。**

**下午中断的历史采样（17:49–17:52）：`684e2a7` 的 SIM-03 初始注册、正常关闭/重启和主进程崩溃恢复已通过；但原自然续期观察中断，不能再等待原定 18:29 的会话续期。设备 uptime 表明约 17:42:51 重新开机，原因未确认，助手未执行重启或切卡。随后日志使用 `45400 / 46001` 派生配置，并出现 IPv6 P-CSCF 路由 `Invalid source address`、QMI endpoint hangup 和 modem 消失；当时 API 为 `registered=false / volte_line_not_present`。见第 4.7 节。当前 SIM-03 已再次上线，但这些旧卡/IPv6 待查证据仍保留，不能因新 IPv4 会话成功就发布 beta4。**

| 范围 | 已完成 | 尚未完成 |
| --- | --- | --- |
| 正式 `1.1.4-beta3` | 已发布、部署；SIM-01、SIM-02 均通过标准派生配置的初始注册与原通道自然续期 | 不代表后续主 QMI 架构改动已兼容这两张卡 |
| catalog / 承载错误处理 | 初始 AKA 基线、可选库缺失、承载失效检测、P-CSCF 轮询已修复 | 最新候选上的多卡、库专属参数回归 |
| QCA410 IMS 接入 | MM/proxy 持有主 IMS bearer，应用独占配置 wwan0 并放入 UE namespace；SIM-03 初始认证/注册、关闭和恢复通过；晚间原 socket 自然续期通过 | 45400 / IPv6 故障待查，SIM-01/02 在新 MM 路径上的回归未通过 |
| 最新真机探针 | 修正 proxy 掩码后能建立 IPv4 IMS 承载，ModemManager 对象稳定，退出后 context 释放 | 探针没有发送 SIP，不能算 IMS 注册成功 |
| 旧候选 `aabe4e5` | 历史 CI/部署完成，但独立 qmicli 路径始终未注册 | 已由新实现替代，不再部署或继续试参数 |
| 最新候选 `684e2a7` | 两条 CI、双架构、受保护部署及 SIM-03 初始注册/关闭/恢复/首次原通道自然续期均通过 | 旧卡/IPv6 及业务回归；不是 beta4 Release |
| 下一版本 | 用户已要求验收后合并 master，统一为 `1.1.4-beta4` | 尚未合并、升版或发布；不能提前宣布修复完成 |

## 2. 项目目标与不可变约束

- 项目实现用户态蜂窝 IMS、VoWiFi、线路级配置、短信/语音/Trunk 等能力。当前优先任务是修复 QCA410 上第三张卡的蜂窝 IMS，并保持已有能力。
- QCA410 的 IMS **只能走主 `/dev/wwanNqmi0` + qmi-proxy**。DATA6 次端点用于普通蜂窝数据；不要把 IMS 再切回 DATA6。
- DATA6 是项目通过 `DATA6_CNTL`、系统 RPMSG 驱动和 `ID_MM_PORT_IGNORE` 等机制创建并隔离的端点，不是“硬件天然提供的 IMS 专用口”。
- 上述是 **QCA410 设备契约**，不能推广成所有硬件的统一实现。保留设备驱动边界、保护性注释和回归测试。
- 不增加要求用户选择 IMS 接口模式的 CLI 参数、环境变量或配置开关。
- 不用任意 USB/QMUX binding 去猜 DATA6 接口。新的源码核对确认：MM 的 QCOM 主承载会依据 BAM-DMUX `dev_port` 执行已知 A2 SIO 绑定，这不能与早期危险的未知接口实验混为一谈。直接复制该绑定也未单独修好本轮 qmicli 路径；当前草稿让 MM 完整管理主 WDS，而不是暴露用户绑定参数。
- **不在本地运行 Rust 编译、测试、打包；使用 GitHub Actions。** 早期本地依赖解析失败不算测试通过。
- 优先修复标准派生兜底；参考 iOS/3GPP 的有据行为，不复制无关 MVNO 身份，不篡改 catalog 的 ready/unknown 状态来制造成功。
- 不自动拨打电话、发送短信、启用 Trunk 或修改资费策略。驻网 home、漫游允许、注册成功都不是免费保证。
- 保留当前配置、数据库、备份、诊断证据、ModemManager、qmi-proxy、DATA6 服务和管理链路。用户明确要求 **不要删除 push key**。
- 注册成功、自然续期、双注册、真实短信/通话是不同验收项。禁止固定 120 秒续期、重建 SA 后重新注册冒充 refresh，或仅改状态标志冒充双注册。
- 修复实机验收后才合并 master、升 beta4、清理已合并开发分支。不得删除尚未合并的工作；beta 必须 pre-release、非 latest，不覆盖已发布 beta3 资产。
- 历史会话权限不自动继承。新环境的联网、写入、部署等仍遵守当次权限要求。

## 3. 仓库与版本基线

| 项目 | 本次本地核对结果 |
| --- | --- |
| 工作区 | `/mnt/d/Program/Learning/AI/ProjectOfRong-lilith/SimAdmin` |
| 当前分支 | `fix/sim02-catalog-aka-baseline`，名称沿用 SIM-02，实际上已包含 SIM-03/QCA410 修复 |
| 程序代码基线（本轮文档提交前 HEAD） | `684e2a71e0227dc66f9b7591ab982485933bde15`；文档提交可能在其上，不能把文档 HEAD 当作已构建的新程序 |
| 当前代码版本 | `1.1.4-beta3`，仍是开发候选，不是 beta4 |
| MM 实现 | `6baba57` 完成生命周期实现；`684e2a7` 修正数字 modem selector，两条 CI 通过且已部署注册。不是未提交草稿 |
| master 基线 | `48612e20b950b05cdf3f575e5bbd02191879e801`；本轮已通过只读 `ls-remote` 核对 GitHub master，未合并或改动它 |
| 正式 beta3 tag / 程序（历史发布记录） | `05de680`；`48612e2` 是后续文档提交，不是发布二进制 commit。本地未取回 `v1.1.4-beta3` tag，未在本轮在线重验 tag |
| GitHub remote | `simmaster` → `git@github.com:autisticryptic/SimMaster.git` |
| origin | Windows 本地仓库路径，不要把它当 GitHub 发布远端 |
| 其它本地分支 | `fix/1.1.4-beta3-cellular-ims` = `07ef05c`；`refactor/1.1.4-beta2` = `48e37fc` |
| 工作树 | 整理开始时 tracked 文件无修改，已有本文草稿及大量未跟踪 `.tmp-*` 等文件；实机续接另增被忽略的诊断/部署脚本，不清理其它文件 |

正式 beta3：[Release](https://github.com/autisticryptic/SimMaster/releases/tag/v1.1.4-beta3)，发布构建 run `34281100059`。不要仅通过版本字符串判断设备是否更新，同为 beta3 的多个候选必须用 commit 和二进制哈希区分。

`VERSION`、`backend/Cargo.toml`、`backend/Cargo.lock` 中的本包版本、`frontend/package.json` 本轮均核对为 `1.1.4-beta3`。本地分支与缓存 upstream 一致，不代表已经重新 fetch 确认服务器状态；远端合并/删除前必须再核对。

## 4. 两段会话的进展

### 4.1 9 月 9 日会话及其延续

下面第 1–4 项是该会话承接的较早基线，证据来自其读取的实机台账及 `plan.md`；该会话开始时已经在测试第三张卡。不是本次重新执行了 SIM-01/02 验收。

1. 在已备份、使用本仓库干净配置/数据库的远程 QCA410 上，修复原生端点准备、AT 紧凑双地址解析、WDS/SIP 地址族不一致问题。
2. 按 TS 24.229 §5.1.1.2.2 修正标准派生 LTE 初始 Authorization 的 AKA 身份。候选 `65b8a0e` 注册并自然续期成功，四库对照完成。
3. 验收后合并并发布正式 beta3 `05de680`，发布包自身再次通过 SIM-01 初始注册和自然续期。
4. SIM-02 在正式 beta3 的标准派生路径注册、自然续期成功；其 Pixel catalog 路径暴露“未指定 initial_authorization 被当作 none”的独立问题，建立当前修复分支。
5. SIM-03 持续未注册。逐步修复可选库缺失错误、失效承载仍发 REGISTER、P-CSCF 只尝试首地址等缺陷。
6. 用户明确 QCA410 主 QMI / DATA6 分工；实现主 QMI + proxy 和长期单一 WDS owner。后续验收目标改为 beta4，而不是重发 beta3。

### 4.2 9 月 12 日会话

1. 取回并部署 `4a5d3b2`，首次因 proxy PID 保护检查触发回滚，第二次成功。
2. 应用仍导致 qmi-proxy 连接丢失和 ModemManager 重探测；旧 modem 对象失效，`CGCONTRDP` 报 `couldn't find modem`。
3. 00:34 暂时通过现有 API 关闭本线路 IMS 连接，防止库存刷新反复触发故障。
4. 提交 `550557e`，修复 follow 输出读取任务过早退出、关闭子进程 stdout/stderr 的问题。
5. 进一步核对设备实际 qmicli 版本及其源码，确认 `--device-open-net` 会覆盖整个 flags 掩码，把先前的 proxy 标志清掉。
6. 修正掩码后，00:38 有界承载探针成功，00:40 确认承载已释放；提交 `aabe4e5`。
7. **00:46:40 后台任务已报告两条 CI 成功，并完成候选下载校验。** 旧台账末尾“等待 CI/下载”已过期，不能当当前阻塞点。
8. 随后的“继续”和 00:54/00:58 中止没有新的部署结果。交互 SSH 进程的最终汇总输出包含较早的设备观测，不能把进程结束时间当作一次新的实测时间。

### 4.3 9 月 12 日上午继续实机工作

- 用户再次明确：SIM-03 已确定能够注册 IMS，并重新提供 `1.1.7-beta8` 参考包；“该卡本身不支持 IMS”不作为本轮假设。参考版本可用与当前分支是否验收通过分开记录。
- 08:25–08:27 经授权 Cloudflare SSH 重新连接，设备/卡摘要与上次一致，仍运行 `4a5d3b2`，主程序 PID、ModemManager 对象 `/Modem/56` 未变，管理默认路由为 wlan0；原生活动通话为 0。
- 部署前 API 确认 IMS 关闭、`volte_line_connection_disabled`、recovery idle；VoWiFi/普通数据/Trunk 关闭，短信 VoWiFi-only=false、Trunk VoWiFi-only=true、漫游允许=true。
- 08:33:16 新候选 `aabe4e5` 部署成功，主服务 PID=1372538、NRestarts=0。包/二进制 SHA256 与第 7 节一致；配置 SHA256=`cbb70ae5482dc9602020a7bca2b774cc0e15608efbcc813fffb972a649b4394b` 在部署前后相同。备份为 `/opt/simadmin-backups/20260912-before-aabe4e5-attempt1`，包含停止应用后的配置/数据库快照及旧程序资产。
- ModemManager=408、DATA6 initializer=966、qmi-proxy=1245504 在部署前后未变，管理出口仍 wlan0；没有重启这些依赖服务。08:35 通过现有 connection API 恢复 IMS，只有连接开关及其 video 镜像恢复为 true，其余上述意愿/费用字段保留。
- 08:37:56 只读实测：qmicli follow PID=1373444，和 proxy 同处宿主 netns，通过 socket 而非直接打开主 QMI；proxy 持有 `/dev/wwan0qmi0`，modem 始终 `/Modem/56`。CID 2 active、IPv4/P-CSCF 正常，`wwan0` 已进入 UE namespace 且两条 P-CSCF 路由正确。
- 初始双栈尝试收到 `(6,50) ipv4-only-allowed` 后按既有逻辑转 IPv4；第一个 derived 槽开始 REGISTER。两个 P-CSCF 被轮询，但截至上述观察无完整 SIP 响应、auth_rounds=0；不将 runtime 的 `volte_register_initial_unexpected_status` 误读成已收到 403，日志实际为 `ims_register_initial_receive_failed`。
- 重新下载用户提供的 beta8，包 SHA256=`ab943a799421d1759d611be089342ba3427382cd8a0c7a9327b5cd4228854bdd`，与历史缓存相同；metadata commit=`930365d`。仅离线检查，未在设备执行或覆盖配置。
- 08:44:14 三槽恢复批次耗尽，三次均为无 SIP 回包后 WDS 退出，最后错误 `volte_bearer_session_lost:qca410_primary_qmi_session_disconnected:pid_exit=exit status: 1`。08:44:33 再查 CID 2 已释放、wwan0 已退回宿主且旧地址/路由被清理；ModemManager、proxy、DATA6 和主程序均未重启。前面的短时承载成功不能写成长期承载已通过。
- 08:42:18–08:42:58 被动抓包在 UE 的 wwan0 看见 13 个发往 P-CSCF 的 REGISTER、0 个回包；宿主其它接口也无对应回包。报文 1444/1491 字节、无 IPv4 分片，包含初始 AKA、Contact audio/MMTEL；这仍不是“大包/MTU 是根因”的证明。
- 管理 SSH 后来中断，第一次暂停 API 未成功；09:14 重连先确认设备仍运行同一进程、IMS 开关仍 true/exhausted，再于 09:15 左右成功暂停。配置恢复为原关闭快照的同一摘要。小报文对照与 WDS 退出原因诊断使用独立有界脚本，不能与应用注册并发。
- 用户补充的 iOS / Pixel 语音差异已列入第 9 节独立验收项，不能被 IMS 注册结果覆盖。

### 4.4 进一步承载对照及代码草稿

这些实验没有运行 beta8 二进制，只按参考资料及设备对应 MM 源码复现承载管理方式；不是重复验证“卡能不能完整注册”。

| 时间 | 对照 | 结果 |
| --- | --- | --- |
| 09:22–09:25 | 独立 qmicli，IPv4，小 REGISTER 约 789 字节 | 两个 P-CSCF 无回包，约两分钟后断开；结束原因仅 generic-unspecified，扩展 type/reason 为 0，不能映射为漫游拒绝 |
| 09:33–09:35 | MM 创建私有 IMS bearer，主 qmi0/wwan0，宿主内发送相同小报文 | 两个 P-CSCF 均立即返回 423，观察末尾 bearer 仍 connected；之后仅清理本次对象、恢复主程序 |
| 09:42–09:44 | 暂停主程序，重复原 qmicli 路径 | 仍无响应、随后断开，排除“仅因主程序后台任务干扰” |
| 10:00–10:09 | qmicli 分别去掉 NET_* 改写、只指定 profile index 不覆盖 APN | 仍失败，不能把任何一项单独写成根因 |
| 10:15–10:18 | 按 MM 源码预分配 CID、A2 SIO 绑定、设置 family，再交给 follow；小/1491 字节报文 | 设置命令成功，但 SIP 仍无回应；不再继续任意增加原始 QMI 参数 |
| 10:26–10:29 | MM 私有 IMS bearer 的 wwan0 移入独立测试 namespace | 两个 P-CSCF 均返回 423、Min-Expires=7200；观察末尾仍 connected，清理后 CID 2 inactive，临时 namespace 删除，依赖 PID 不变 |

设备是 MM `1.18.4`、BAM-DMUX，`wwan0/dev_port=0`。对应公开源码为 MM 的 `qcom-soc` port mapping 和 `mm-bearer-qmi.c`。已确认有效的是**完整的 MM 承载路径**，没有证明某一个原始 QMI TLV 是唯一差异。

工作区草稿：

- 新增 `backend/src/hardware/devices/qcm410/primary_ims_session.rs`：仅创建自己的 IMS bearer，校验 modem 主控制口、APN、数据接口及其它 bearer 占用；禁止接管 CreateBearer 返回的既有对象。
- MM/proxy 长期持有 WDS，应用继续配置并隔离专属数据网卡；状态观察与本对象 Disconnect/Delete 配对。DATA6 普通数据路径不改，不增加用户模式开关。
- 将已有 `BearerRequest.allow_roaming` 转交设备 transport，保持用户策略，不在驱动内擅自放开漫游。
- 已写所有权、错误回滚、状态观察、输入/脱敏回归，并加入两条 Actions 测试过滤器。**只做过格式/静态检查，新增 Rust 测试尚未编译执行。**
- 现场额外发现 `mmcli 1.18.4 -m ... --list-bearers` 返回 `no actions specified`；草稿已改为解析 `-K` 中的 `modem.generic.bearers*`，排除单独的 Initial EPS attachment。不能以模拟测试代替这个实机 CLI 契约检查。
- 11:21:10 收尾只读核对：`modem.generic.bearers : --`，无遗留 qmicli 或测试 namespace，宿主 wwan0 无残留 IPv4 地址；配置摘要仍为关闭快照。主服务 active/PID=1401375，MM=408、DATA6=966 保留。当前并没有后台运行中的注册验收。

**草稿下一项必须先处理：**旧 `main.rs` 的进程退出只显式释放 DATA，随后会 `process::exit`；MM bearer 不会随 SimAdmin/qmicli 子进程退出自动释放。因此要补齐正常 shutdown、注册建立中取消及遗留资源处理，并考虑 MM 重启后的对象编号复用，不能删除新进程/其它程序的 bearer。此项收口及 CI 通过前，不部署草稿、不合并/发布 beta4。

### 4.5 生命周期实现、CI、部署及路径适配修正

本节更新覆盖第 4.4 节的“草稿未提交”暂停点：

- `6baba57cbc4487d3962921cdaea1baad48da5653` 已提交并推送当前修复分支。控制操作改用绑定 **D-Bus unique owner** 的调用，避免 MM 重启后相同对象路径指向其它新 bearer。
- `/run/simadmin/primary-ims-owned/` 保存本程序创建的 bearer 归属；记录在 Connect/网卡配置/namespace 移动前原子写入，目录 0700、文件 0600，不存 SIM 身份或认证材料。恢复时核对 bus ID、unique owner、进程 PID/启动时间；不接管另一个仍活着的实例。
- 取消中的 setup 会继续收取结果并释放资源；网络变更 guard 阻止 cleanup 越过未结束的 Connect/网卡移动。进程 shutdown 有独立的有界 IMS 清理，遗留账本在下次启动恢复，DATA6 和其它硬件不套用 QCA410 实现。
- 本地仅格式/diff 和 18 项 Python 发布规则测试；Rust 编译/回归由 Actions 完成。`Validate Beta Refactor` [34676929089](https://github.com/autisticryptic/SimMaster/actions/runs/34676929089)、`Build-Release` [34676929103](https://github.com/autisticryptic/SimMaster/actions/runs/34676929103) 全通过，发布 job skipped。
- 6baba57 包 SHA256=`f533f14def3aae1d8749a2035fc06185233c22eba653e33c839614e62fb8eab2`，二进制 SHA256=`ac2f6eb9000907508482eac58a2c68abf866df7e538c452d7007102f69a6268a`。15:11:09 部署成功，主程序 PID=1474382，备份 `/opt/simadmin-backups/20260912-before-6baba57-attempt1`。
- 配置摘要部署前后为同一 `cbb70a...4394b`；MM=408、DATA6=966、proxy=1245504 不变，管理出口 wlan0。恢复 IMS 后，实际错误为 `qca410_primary_mm_dbus_failed:Invalid object path`；发生在创建承载前，不能当作网络/SIM 注册拒绝。
- 根因是既有 IMS 层给 mmcli 传数字 selector（如 `56`），而 D-Bus 需要 `/org/freedesktop/ModemManager1/Modem/56`。`684e2a7` 在 MM 适配器内部规范化两种形式，新增非法值/数字/完整路径回归，不改其它设备或用户线路标识。
- 新 CI：Validate [34681227857](https://github.com/autisticryptic/SimMaster/actions/runs/34681227857)，Build [34681227851](https://github.com/autisticryptic/SimMaster/actions/runs/34681227851)。此刻等待结果；不可把 6baba57 的通过状态当作 684e2a7 已通过。

### 4.6 整程序注册与生命周期实测通过，下午续期观察后来中断

- `684e2a7` 的上述两条 CI 均 success，含新增回归及 arm64/amd64；发布 job skipped。16:02:14 部署成功，包与程序哈希见第 7 节，配置、数据库和依赖 PID 保留。
- **16:08:18 第一次整程序注册成功**：`derived_3gpp_lte_45403`、首槽、IPv4/UDP、主 `/dev/wwan0qmi0`、UE 内 `wwan0`。日志记录认证 challenge，随后 REGISTER 成功；lease=7200、refresh_after=6600，未人为改租期。
- 网络回包关联身份数=2、Contact binding=1；无 Security-Server，实际是 UDP，不是 IPsec；Service-Route=0、voice_service=unknown。本端 offered outbound，但网络未确认，因此不是双注册/语音业务验收。
- **16:27:58–16:27:59 正常关闭检查通过**：停止主服务后，本对象 `/Bearer/48` 删除、ledger 清空、wwan0 返回宿主且 IMS IPv4 地址去除、CID 2 inactive；配置哈希及 MM/proxy/DATA6 PID 不变。停机期间另保存一致性数据库/配置备份 `/opt/simadmin-backups/20260912-before-mm-lifecycle-tests`。
- 16:29:59 主服务恢复后自动重新注册成功。它是重启后的新初始注册，**不是自然续期**。
- **16:37:21–16:37:35 主进程异常退出恢复通过**：确认无应用/原生通话且已有一致性备份，仅对 simadmin.service 的 MainPID 发送 SIGKILL；MM、proxy、DATA6 不动。新进程自动回收旧 ledger 和 `/Bearer/51`，配置保持原样。
- 最终进程 PID=`1497825`，NRestarts=`1` 来自上述人工故障注入，不是自行崩溃。**16:39:27 再次自动注册成功**，当前 `/Bearer/54`、PCSCF `10.5.236.166:5060`，session_started_at=16:38:37，reconnect_count=1，refresh_count=0。
- 从该最终会话开始只读观察。目标是约 **18:29:27** 的原通道自然续期，必须保持 session_started_at、reconnect_count 与 transport 不变；前三次初始注册均不能冒充 refresh。不要在观察期间切库、改配置、缩短租期、部署或重启。
- 当前环境没有 Python3、jq、sqlite3，且连 Perl JSON::PP 也没有；辅助台账检查已改成无新增依赖的受限字段读取。首次诊断脚本缺模块退出不属于应用故障，未安装系统包。

### 4.7 原续期观察中断及新启动故障（17:49–17:52 只读复核）

本节覆盖第 4.6、8、9 节中“原会话正在等待 18:29 续期”的实时状态描述；保留旧记录用于追溯，不能重放旧测试脚本。

- 原观察最后有效样本为北京时间 **17:38:20**：SIM-03 原会话仍 registered，reconnect_count=1、refresh_count=0。之后 SSH 观察器报 SSHException，本地汇总器因数据过期退出。没有已确认的自然续期成功证据。
- 只读重新连接于 **17:49:01**：设备 uptime=370.18 秒，即约 **17:42:51** 开机；simadmin MainPID=451，而不是旧会话的 1497825。重启原因未知，助手本轮只做连接、授权 Web 登录和只读查询，没有重启、切卡、改配置或触发 IMS retry。
- 17:52:45 诊断确认设备仍为 `1.1.4-beta3 / 684e2a7`。主服务 PID=451/NRestarts=0；ModemManager PID=2230；DATA6 initializer PID=1539/NRestarts=2。这些是新启动的状态，不能继续声称旧依赖 PID 未变。
- API 同一 line_id 当前 `present=false`、`registered=false`、`phase=disabled`、`last_error=volte_line_not_present`；IMS 开关仍 true。此时 mmcli 枚举不到 modem，未发现 UE namespace 或 qmi-proxy/qmicli 进程。
- 新启动日志在 **17:45:34** 选择 `derived_3gpp_lte_45400`，home PLMN=45400、visited PLMN=46001，与历史 SIM-01 的网络组合一致。**尚未由用户确认是否换卡，不能只靠缓存的 operator_id 认定卡片身份。**
- **17:45:37**：新 MM bearer 返回 IPv6 数据配置、wwan0 进入 UE；安装两条 P-CSCF 路由均报 `Invalid source address`。**17:45:38**：MM 记录主 QMI endpoint hangup，modem 对象消失，清理旧对象遇到 UnknownMethod 并保留归属账本。此后 line not present。先记录时序，不把路由失败、基带掉线及重启未经验证地归成单一根因。
- 原 SIM-03 连续自然续期测试应记为 **中断／未验收**，不是通过，也不能据此认定为 SIM-03 续期失败。需要先确认重启/卡片变化和当前设备状态，再决定恢复哪张卡的验收；不自动反复重启 MM、DATA6 或主服务。
- 远程新客户端会话仅用于只读检查，未重新启动自然续期 watch；旧本地汇总器已退出。跨对话不要假定旧观察器仍在工作。

### 4.8 “ModemManager 管理权”的准确含义

- 正式 beta3 `05de680` 的 QCA410 IMS 使用项目创建的 DATA6 备用 QMI 端点；`secondary_qmi_init.rs` 对该备用端口/对应网卡设置 `ID_MM_PORT_IGNORE=1`。这是让 MM 不枚举、接管这些专用端口的职责隔离，**不是撤销 MM 对整个 modem 的权限或停掉 MM**；主 QMI 仍留给 MM。
- 随后的修复中间版 `aabe4e5` 已把 IMS 改到主 QMI，使用长期 qmicli WDS session + qmi-proxy。proxy 只负责复用控制口，不意味着该 WDS session 已成为 MM 自己创建、管理的 D-Bus bearer。此前旧 qmicli 的 flags parser 清掉 proxy 标志是独立的意外直开主口缺陷，不是有意“夺取管理权限”。
- 当前 `6baba57 / 684e2a7` 由 SimAdmin 的 QCA410 驱动通过 D-Bus 请求 MM 创建、连接和维护一个新私有 IMS bearer。MM 管底层 WDS 生命周期；SimAdmin 保留该 bearer 的归属并负责释放，专属数据接口仍交给 UE，SIP/AKA/续期仍在原应用协议栈。
- DATA6 的端口隔离不因此被全局撤销，当前只供普通蜂窝数据使用。控制面由 MM 管理与数据面网卡进入 UE namespace 可以同时成立。
- 这是对底层承载职责的调整，不是取消 UE 隔离。SIM-03 已验证的范围与第 4.7 节的新 IPv6 故障必须分别报告，不能以架构边界保留推导所有卡已经兼容。

### 4.9 入口不可达，未能继续读取重启原因（18:38–18:48）

- 用户说明设备属于朋友，目前没有收到朋友是否重启/换卡的消息，并要求继续检查。不能把“用户不知道”改写为“已确认无人操作”。
- 原 SSH 的只读 baseline 请求返回 SSHException；随后两次使用已有授权凭据建立新 WebSocket，均收到 HTTP 530。第二次失败时间为北京时间 **18:48:02**，已从响应中仅提取数值 **Cloudflare Error 1033**，不保存/打印 Cookie、响应头或整页内容。
- 失败发生在 Cloudflare 层，**没有进入 SSH 认证，更没有在设备上执行任何新诊断、重启、修改配置或 retry**。1033 支持“隧道连接当前不可用”，不能区分断电、设备网络故障、cloudflared 停止或隧道配置/服务问题；也不能据此认定 SSH 密码错误。
- 最后可读的设备快照仍是第 4.7 节的 17:52 结果，不代表 18:48 的内部状态。SIM-03 原续期、是否换卡、整机重启原因、IPv6 路由错误与 QMI 掉线的因果关系仍未验收/确认。
- 已准备 `.codex-cf-reboot-investigation-20260912.sh`，本地 `sh -n` 通过，**尚未在设备执行**。恢复入口后只读采集 boot history、上次关机日志、kernel/remoteproc 状态、MM/SIM 脱敏指纹、自动恢复服务记录及当前 UE 接口，不开启新 bearer、不重启任何服务、不安装辅助软件。
- 源码额外发现已有 `simadmin-modem-recovery.service/timer`：异常时可自动重新通知 QCOM 端口，并最多在一次恢复运行中重启一次 MM；源码明确不重启 MPSS/操作系统。因此 **MM PID 改变不等于朋友手工重启 MM**。是否本次实际触发需要读取设备日志，当前不能以源码代替现场证据，也不能用它解释整机 uptime 重置。
- 下一步先恢复/确认 Cloudflare 隧道和设备管理网络，只读保全新故障证据；不要为了排查入口故障先重启基带、清理账本或回放旧部署脚本。新的连接失败证据已存 `.codex-cfssh-results/connection-failure.*.json`。

### 4.10 SIM-03 晚间重新上线和观察基线（续期结论见 4.11）

本节覆盖第 4.9 节的入口不可达状态。用户明确说当前已是第三张卡，并要求继续 IMS 验证；同时要求版本规划文档随后一并提交 GitHub。

- **19:19:37** 只读连接成功：设备 uptime=487.43 秒，主服务 PID=443。**19:25:13** 诊断确认 `1.1.4-beta3 / 684e2a7`、aarch64；未重新部署、修改配置或触发注册。
- MM 当前 `/Modem/2`，SIM 属性的 IMSI 前五位为 45403，访问网 46000，与 SIM-03 相符。完整身份只在设备内存中处理，保存的辅助证据仅有脱敏/摘要信息，不将身份明文纳入文档。
- 自动新会话 session_started_at=`2026-09-12T11:13:22...Z`（19:13:22），registered_at=`2026-09-12T11:14:13...Z`（19:14:13）。日志有认证 challenge，随后初始 REGISTER 成功，profile=`derived_3gpp_lte_45403`、derived 首槽、IPv4/UDP。
- 此次双栈/IPv6 尝试因活动 IMS context 无 IPv6 地址按既有顺序退出，然后 IPv4 成功；未全局强制 IPv4。网络租期 **7200 秒**，正常 refresh_after **6600 秒**，首次自然续期预计北京时间 **21:04:13 左右**。
- 初始成功元数据：关联身份 2、Contact binding 1、Service-Route 0，网络未提供 Security-Server，也未确认 outbound。因此仍不能宣称 IPsec、双注册或呼入/呼出业务通过；语音服务为 unknown。
- **19:27:25** 归属检查：SimAdmin PID=443/NRestarts=0，MM=433/NRestarts=0，proxy=655，DATA6=662/NRestarts=1。唯一 MM bearer 为 `/Bearer/3`，connected/IMS/IPv4；归属账本 1 个，目录 0700、文件 0600，owner=`:1.10`、process_id=443、namespace=`sa-ue286e0c9d2870`，网络配置已记录。管理默认路由仍 wlan0。
- **19:30:13** 开始每 120 秒只读观察，基线 reconnect_count=1、register_refresh_count=0。本地汇总器严格核对线路、session_started_at、registered_at、profile、IPv4/UDP、P-CSCF 和重连计数。不能使用旧的固定 16:39 基线汇总器。
- **19:46:58** 补充只读证据：设备二进制 SHA256 与第 7 节的 `18408e9b...58c167` 完全一致；主进程/依赖、唯一账本及 Bearer/3 保持。UDP tuple 为 `10.26.25.247:5060 → 10.5.236.166:5060`，socket 归 MainPID=443/FD=19，namespace inode=4026532440；后续用同一脚本核对续期前后 tuple/FD/namespace，不仅看注册状态布尔值。
- 观察启动时使用 `.codex-cfssh-client.py` 的 `watch`（最多 65 次、120 秒、until_refresh_increment=true）；汇总器为 `.codex-cf-natural-summary-current.py`，期望时间前缀分别为 `2026-09-12T11:13:22.` / `2026-09-12T11:14:13.`。脚本只读；计数增加后已核对 journal、原 bearer 和进程，最终结论见第 4.11 节。

**启动诊断边界：**

- 已执行先前准备的只读 reboot-investigation 脚本。当前 kernel 日志在 **19:11:56** 报 `THIS IS INTENTIONAL RESET, NO RAMDUMP EXPECTED`，随后 remoteproc0 自动恢复、DATA6 initializer 因端口消失重启一次；这些发生在本次 IMS 建立之前。该报文不能证明是朋友手工操作，具体触发者仍未知。
- **19:13:05–19:13:24** 自动 modem recovery 服务实际执行了针对性 QCOM 端口重新通知并恢复 MM 枚举；没有走重启 MM 分支。之后周期检查均为 healthy/no recovery needed。此项有日志证据，不再只是源码推测。
- 系统早期 wall-clock 日志显示 17:51，随后跳到 19:11，wtmp 中还有 1970 时间。应结合 boot ID、uptime 和校时后的事件，不据旧墙钟时间计算注册耗时或认定重启原因。当前时间与 uptime 折算约 19:11:30 开机；上次中断的完整关机原因仍无足够日志证明。
- 下午 45400 / IPv6 的 `Invalid source address` 与 modem 丢失仍是独立未解决项；现在 SIM-03 IPv4 成功不代表该回归已修好。

### 4.11 SIM-03 原会话自然续期验收通过（21:04–21:10）

**验收范围：当前 `684e2a7`、SIM-03、标准派生首槽、IPv4/UDP 的初始注册及一次自然续期。** 不扩称为所有卡、四库专属参数、IPsec、双注册或真实通话已通过。

| 证据 | 结果 |
| --- | --- |
| 初始注册 / 会话 | registered_at=19:14:13，session_started_at=19:13:22，续期后两者不变 |
| 续期请求 | 21:04:14.418477，REGISTER CSeq=4；初始注册最后 CSeq=3，续期单调增加 |
| 网络成功结果 | 21:04:14.830347 日志明确 `register_phase="refresh"`，新租期 7200 秒；关联身份 2、Contact binding 1 |
| 后续调度 | 21:04:14.925854 记录成功后重新安排 refresh_after=6600、lease=7200，未使用测试短租期 |
| API / 观察器 | 19:30:13–21:05:23 共 48 次只读样本，registered 始终 true，reconnect 始终 1，refresh 从 0 增到 1；21:10 API 仍注册 |
| 程序与系统 | 二进制 SHA256 与第 7 节一致；boot reference、MainPID=443、MM=433、proxy=655、DATA6=662 及各 NRestarts 不变 |
| MM 与归属 | 同一 `/Modem/2`、`/Bearer/3`、owner `:1.10` 和唯一 lease 文件；未删除/重建归属 |
| UE 网络 | 同一 `sa-ue286e0c9d2870`，namespace inode=4026532440，wwan0/源地址/路由不变 |
| UDP 通道 | `10.26.25.247:5060 → 10.5.236.166:5060`，MainPID=443/FD=19，socket inode=12411，续期前后完全一致 |
| 失败/重建 | 本次续期日志未出现 retry/rebuild，API 记录 `register_refresh / succeeded / cseq=4` |

- 19:51:34 的续期前快照与 21:06:28 的续期后快照做了字段级比较，不能仅凭页面的 registered 标志得出上述结论。
- 21:15:28 再读 MM 与归属：`/Bearer/3` 仍 connected、IMS/IPv4，唯一 ledger、owner、进程、namespace 及管理路由不变；没有把遗留账本误当作活动承载。
- 观察器在计数增加后正常结束，本地汇总器退出码 0；没有停止 IMS，也没有为测试重启、retry、换卡、切库或改配置。
- 原下午 16:39 会话仍记为“观察中断”，不能将这次成功回填成那个会话已续期。正常关闭/崩溃恢复的下午实测记录继续独立保留。
- 当前网络仍无 Service-Route、无 Security-Server、未协商 outbound，voice_service=unknown；本次为 UDP 注册/续期，不是通话、IPsec 或双注册验收。
- 后续需要 SIM-01/02 的新 MM 路径回归，特别是 45400 / IPv6 的地址/路由及 modem 消失问题；Pixel 呼入语音信箱继续独立排查。暂不合并 master、发布 beta4 或实施 1.1.5/1.1.6 后端重构。

本地脱敏证据（未提交原始观测/脚本）：

- 汇总结论：`.codex-cfssh-results/sim03-natural-refresh-684e2a7-20260912-evening.verified.json`。
- 原 watcher verdict：`natural-refresh-verdict.1789218323420155953.json`。
- 首/末样本：`ims-observation.1789212613726697468.json` / `ims-observation.1789218323406834053.json`。
- 续期前/后：`.codex-cf-s03-evening-refresh-evidence.sh.1789213894072395784.json` / `.codex-cf-s03-evening-refresh-evidence.sh.1789218387861948457.json`。
- 上述文件均在 `.codex-cfssh-results/`；核对时同时检查晚间 session/profile，不能使用旧 SIM 或旧会话的同名类文件。

## 5. 多 SIM / 多库验收矩阵

下表 PLMN 仅为网络标识，不包含完整 IMSI/ICCID。日期均为 2026 年。

| SIM | 归属 / 服务网 | 已证明 | 未证明 / 后续 |
| --- | --- | --- | --- |
| SIM-01 | `45400` / `46001` | 正式 beta3 `05de680`，9/9 05:47:22 初始注册，06:37:24 原 UDP 通道自然续期；IPv6、标准派生。此前 `65b8a0e` 亦独立通过 | 四库均最终依赖 derived，不是四套库专属参数都通过；需在最新主 QMI 实现上复测 |
| SIM-02 | `46000` / `46000` | 正式 beta3，9/9 10:18:45 标准派生注册，11:08:46 自然续期 | Pixel ready 配置曾在 AKA 前被 403 拒绝，提示 `Terminal has used different algorithm from initial register`；缺省 AKA 基线已修，最新候选尚未实测 |
| SIM-03 | `45403` / `46000` | `684e2a7` 标准派生首槽完成认证和注册，IPv4/UDP；正常关闭/恢复、主进程异常恢复通过；9/12 21:04:14 原 socket 首次自然续期通过 | 真实呼入/呼出、短信、双注册及库专属参数仍独立待验收；不是其它卡回归通过 |

四库来自 `autisticryptic/carrier_Bundles` 的 `v0.3.0-catalog-v7` 发布，使用 sealed v7：
`ios-ipcc`、`iPhone16ProMax26.6.1`、`Pixel Mustang`、`Xiaomi15Ultra Xuanyuan`。
不要用本地旧 `26.6` 代替 `26.6.1`；必须分别记录 requested/effective profile、fallback 原因和实际接入参数。

SIM-03 的历史静态库核对：Pixel 的 `profile-h3-hk-45403-84c6302cf9`、Xiaomi 的 `profile-hutchison-hk-ims-45403-84c6302cf9` 有 ready LTE 投影；两套 iOS 库的通用 Hutchison 项是 LTE unknown，不能强行当 ready 使用。其它 HKBN/MVNO 项有额外 SIM 匹配条件，不能仅凭同 PLMN 冒用。**静态查库不等于四库在 SIM-03 上都做过实测。**

设备最后激活的是 Pixel 库，SHA256 为 `10b603c2b29c6fbb08a8f2e796c3db878671051eae97ff734004310fb8709efc`。用户提供的[旧版 `1.1.7-beta8` 对照包](https://github.com/lilith-rong/SimAdmin-Enhance/blob/Backup-Vowifi-and-VoLTE/Volte/simadmin_1.1.7-beta8.tar.gz)属于另一条历史版本线，其“可注册、可接短信”是用户反馈；可用于架构差异分析，不能算当前代码复测通过，也不要直接覆盖现有安装。

历史 beta1/beta2 已完成命名/DNS 重构、自动双注册/仅单注册设置、资费门禁以及另一台设备的 VoWiFi 原通道续期，详见 [plan.md](../plan.md)。这些不是本台远程 QCA410 或最新候选的实测证据。现有网络未确认 Outbound 多注册，不等于运营商永久不支持，也不能宣布双注册目标完成。本轮候选的真实电话、短信、账单和完整浏览器交互仍没有验收结论。

## 6. 关键修复与归因边界

| 提交 | 作用 | 验证边界 |
| --- | --- | --- |
| `bc0a960`、`65b8a0e` | 端点/地址族修复；标准派生 LTE 初始 AKA 身份 | 已进入正式 beta3，SIM-01/02 的派生路径有成功证据 |
| `d7d7998` | LTE catalog 未指定初始认证时使用 `aka_empty` 基线；保留显式 `none` 和 VoWiFi 原基线 | 不代表具体 catalog 已在最新真机通过 |
| `3ad6f7b`、`33c0645`、`ef0121c` | 正确分类 source-bound 可选 catalog 缺失，最终覆盖 VoWiFi 引用路径 | 不把可选来源缺失升级为不可恢复错误，也不吞掉其它真实错误 |
| `39f1821` | WDS owner 死亡后中止注册并报告 `volte_bearer_session_lost` | 设备证明不再沿失效地址继续发 REGISTER；未解决所有承载退出根因 |
| `b42f8f3` | 同地址族 P-CSCF 按序去重轮询；初始两次无响应后切下一地址 | 真机见 1/2 → 2/2，但仍未注册；不得借轮询绕过明确 SIP 拒绝、AKA 错误或承载死亡 |
| `a5de98b`、`aaf160e` | QCA410 固定主 QMI IMS 路径及回归修正，保护 DATA6 分工和其它硬件边界 | 最新整体实现尚需多卡回归 |
| `d0d2891` | 主 QMI/proxy 复用的中间尝试 | 不是最终建议；其错误归因需按下文修正 |
| `e9fa731`、`509a2f4`、`4a5d3b2` | 长期单一 follow owner、Rust 输出 reader 所有权修复、DATA6 文档澄清 | `4a5d3b2` 已部署但仍受错误 flags 掩码影响 |
| `550557e` | startup receiver 丢弃后仍持续 drain stdout/stderr 至 EOF，不记录原始输出 | 已确认代码缺陷；不能声称它是此前每次断线的唯一根因 |
| `aabe4e5` | 修正旧 parser 覆盖 proxy 位 | 历史过渡修复，独立 qmicli 路径仍不通，已替代 |
| `6baba57` | MM unique-owner 承载、取消处理、原子归属记录、namespace/config guard、退出/恢复 | CI 通过，首次实机暴露数字 modem selector 适配遗漏 |
| `684e2a7` | 将既有数字 selector 转为真实 D-Bus modem 对象路径 | CI、SIM-03 完整初始注册、关闭、异常恢复及晚间首次原 socket 自然续期通过 |

### 6.1 最重要的 qmicli 兼容性结论

设备为 qmicli `1.28.6` / `libqmi-utils 1.28.6-1`，动态库 `libqmi-glib5 1.30.2-1mobian1`。不能只看动态库版本来判断 CLI 行为。

以下是**内部参数说明，不是让用户直接在运行中的设备重复探测的命令**：

```text
旧参数：
--device-open-proxy --device-open-net=net-raw-ip|net-no-qos-header

修正后：
--device-open-proxy --device-open-net=net-raw-ip|net-no-qos-header|proxy
```

qmicli 1.28.6 先设置 proxy，再调用 flags parser；parser 首句 `*out = 0` 清空整个掩码。因此旧命令“文字上有 proxy”，运行时却可能直接打开主 QMI，扰动 ModemManager。这里只修改 QCA410 主 IMS 内部参数；不要把 DATA6 的 `--device-open-qmi` 或 direct-open 掩码复制过来。若在 shell 中审查/重现，含 `|` 的整个参数必须正确引用，不能被 shell 解释为管道。

公开源码：[qmicli.c](https://github.com/linux-mobile-broadband/libqmi/blob/1.28.6/src/qmicli/qmicli.c)、[qmicli-helpers.c](https://github.com/linux-mobile-broadband/libqmi/blob/1.28.6/src/qmicli/qmicli-helpers.c)。

00:38:11 至 00:38:24 的一次有界探针：同一 follow 进程建立 IPv4 IMS，`CGACT: 2,1`；P-CSCF 为 `10.5.236.174` / `10.5.236.166`。qmicli FD 显示 socket，没有直接打开主 QMI 的 FD；modem 对象前、中、后均为 `/Modem/56`。没有发 SIP、短信或拨号。SIGINT 的 `operation cancelled` 本身不证明释放成功；00:40:42 另查到 `CGACT: 2,0`、无地址、无遗留 follow，才确认 context 已释放。

### 6.2 必须撤回或避免的旧推断

- 不能把 SIM-03 故障定性为漫游拒绝、运营商拒绝或“不支持这张卡”：未收到相应 SIP 拒绝，且已发现明确软件缺陷。
- 不能说“qmi-proxy 天生不能跨短进程复用 retained CID”：旧实验没有真正保住 proxy 标志，不支持该结论。
- 单一长期 WDS owner 仍保留，理由是它为 liveness/teardown 提供明确边界，不是因为上述 CID 不兼容性已被证明。
- `aabe4e5` 的 follow owner 已完成实测，但仍无信令回包；新草稿改为 MM 长期持有私有 IMS bearer。禁止把短进程退出、MM 管理对象和应用退出混为同一种生命周期，也禁止任意接管未归属的 CID/bearer。
- 承载建成不等于 SIP 可达，SIP 200 不等于原通道续期，单路续期不等于双注册或实际业务通过。

### 6.3 控制端点、数据网卡和旧抓包证据不要混用

- **主 QMI 控制端点**仍由 qmi-proxy/ModemManager 共享管理；**数据网卡**由当前设备驱动映射 `/dev/wwanNqmi0 → wwanN`，经 `resolve_exact` 选择并交给对应 UE namespace。这两类资源不是同一个“口”。
- 最新实现不再为主 IMS 从备用 `wwan1…` 中猜选接口。更早“IMS 在 wwan1、wwan0 留给普通数据”的 DATA6 实验布局已经过期；也不能把“保护管理链路”误解成主 IMS 数据网卡永远不得进 namespace。实际迁移、路由和清理仍须在最新候选验证。
- DATA6 缺失不能在普通数据已关闭时阻塞主 QMI IMS；普通数据启用时仍须具备自己的 DATA6 前置条件，不能借用主 IMS 接口。未知设备不能偷偷退回宿主 namespace。
- 早期旧路径抓包看到了 REGISTER 发出而无回包，较小的 1283/1342 字节变体也无响应；不能认定仅仅是超过 MTU。bam-dmux 计数器恒零不等于没发包，ICMP/TCP 无回应也不等于 IMS 被封禁。这些都不是最新主 QMI 数据面的验证。
- AT 的 PDP context CID（历史 IMS 为 `2`）与 QMI WDS client CID 是不同标识。只读查询用动态 modem 对象，不能将两者混用或复用上次的临时 client ID。

### 6.4 规范与错误分层

`volte_bearer_session_lost` 是应用错误分类，不是 SIP 状态码或一个已确定的 3GPP 拒绝原因：

| 现象 / 层级 | 应查依据 |
| --- | --- |
| LTE PDN / EPS bearer 释放 | 3GPP TS 24.301 的 ESM cause |
| 传统 PDP / PS 域释放 | 3GPP TS 24.008 的 SM cause |
| 5G PDU session 释放 | 3GPP TS 24.501 的 5GSM cause；当前 QCA410 实测不能证明 VoNR |
| IMS REGISTER、P-CSCF、初始 AKA | TS 24.229；初始身份基线见 §5.1.1.2.2 |
| Outbound 多流资格 | RFC 5626 与 TS 24.229；不能只看本端 offered=true |

QMI `network_disconnected`、进程退出或 `AT+CEER` 空结果都不能直接映射为某个网络拒绝码。需要真实 call-end reason/NAS 证据；已发现的本地端点与 flags 缺陷应先解决，不再沿用“只能等运营商开通”的旧排序。

### 6.5 ModemManager 解耦的可行性与范围（版本决策见 6.6）

**技术上可行，但当前没有完整替代实现。** 最初用户询问可行性，随后明确要求形成 1.1.5 双后端、1.1.6 完全原生接管的版本规划，见第 6.6 节。这不是立即卸载 MM 的操作请求；目前只有源码审阅和文档，没有创建重构分支、改生产代码、停服务或改安装依赖。

当前依赖不止新 IMS bearer：

| 范围 | 当前证据 | 替代时需保留的能力 |
| --- | --- | --- |
| 设备发现、线路/槽位、SIM 身份 | `hardware/cellular/modem_manager.rs`、`services/line_registry.rs` | 基于物理设备/槽位的稳定身份、热插拔、SIM/eSIM 切换；不能把 MM 临时对象路径继续当通用硬件 ID |
| 蜂窝驻网、信号、网络模式、运营商选择、飞行模式 | `hardware/cellular/modem_manager.rs`、API handlers | QMI NAS/DMS 或对应 MBIM/AT 实现、状态通知、超时与恢复 |
| IMS/普通数据 bearer | 通用 `cellular_ims/bearer.rs`、QCA410 `primary_ims_session.rs` | WDS 客户端、设备绑定、IPv4/IPv6、APN/漫游策略、P-CSCF、长期状态、取消/清理/恢复 |
| SIM/AT/UIM 访问的定位与协调 | `modem_manager.rs`、`serial.rs`、`at_session.rs`、已有 QMI-UIM 路径 | 保留已有直连部分，但补齐独立发现、命令串行化、SIM/APDU/AKA 访问和端口归属 |
| 原生/CS 短信、原生电话控制 | `services/messaging/sms_listener.rs`、MM Messaging/Voice 接口 | 收件通知、PDU/存储、原生呼叫状态等；不能与用户态 IMS 通话混为一项 |
| 启动、安装和恢复 | `main.rs`、`scripts/simadmin.service`、`install_latest.sh`、QCA410 recovery service/timer | MM debug override、systemd Wants/After、包依赖和恢复逻辑均需按 backend 能力调整 |

已有可复用基础：

- `ImsBearerTransport` / `ImsBearerHandle`、设备 driver 边界、UE worker/netns 及用户态 SIP/IMS 协议栈，不需要因为去 MM 而全部推翻。
- 已有 QMI-UIM、直接 AT 会话和 QCA410 数据路径代码，但“有部分直连代码”不等于已有可替代 MM 的完整设备管理实现。
- 非基带线路已有 MM discovery 失败时继续处理的分支，例如 PC/SC 读卡器；不能笼统说项目每一种功能都必须有 MM。当前主要蜂窝 modem 路径仍依赖它。**去 MM 也不等于去 system D-Bus、libqmi 或 qmi-proxy。**

迁移路线概要（尚未实现，执行明细以第 6.6 节链接的版本规划为准）：

1. 先将发现、SIM/无线状态、AT/UIM、bearer、原生消息/呼叫等能力抽成硬件无关 provider 接口，MM 成为其中一个实现，保留原 UE/线路边界及配置策略。
2. 为 QCA410 单独实现常驻、统一协调的 direct-QMI provider，可复用 libqmi；不能简单恢复已失败的“一组 qmicli 命令”，需要承担 MM 目前完成的设备绑定、事件、生命周期和恢复职责。
3. 与 MM provider 做同卡、同配置回归：SIM-01/02/03、IPv4/IPv6、自然续期、多 UE、取消/崩溃、热插拔、SIM/eSIM 切换及所支持的业务；其它 MBIM/AT 设备分别适配，不能强加 QCA410 契约。
4. 1.1.5 使安装/启动的 MM 依赖可选；1.1.6 按已确认目标完全删除 MM 后端、调用与依赖。原生覆盖和回归不足应阻止发布，不保留隐藏 fallback。

收益是减少外部 daemon 依赖、集中资源归属；代价是将其兼容性、异步状态机和恢复维护成本转移给项目。**依赖更少不自动等于更鲁棒，也不能保证去 MM 会修复当前未定位的 IPv6/基带问题。** 建议与本次 SIM-03/旧卡修复验收分开推进。

### 6.6 用户确认的 1.1.5 / 1.1.6 版本规划

- 新增独立文档：[设备后端版本规划](MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md)，并接入 `DEVELOPMENT_PLAN.md` 总入口、README 文档导航及架构/driver 说明。跨环境继续本任务时携带该文件，不只携带本文。
- **1.1.5**：统一设备能力接口，同时支持 MM provider 与原生 QMI/MBIM/AT；MM 变为可选依赖。双后端可以服务不同设备，不能共同管理同一物理 modem。
- **1.1.6**：移除 MM provider、运行调用、必装依赖及专属恢复逻辑，原生接管所有明确承诺支持的设备能力；不保留环境开关、自动安装或隐藏 MM 回退。
- 规划包含能力范围、M0–M5/N0–N5 阶段、具体源码边界、设备矩阵、同卡对照、自然续期/长稳、多 UE、迁移和包级回滚。硬件/能力未达标会阻止发布，不能静默缩小支持范围。
- 版本规划阶段仅文档变更，`VERSION` 仍为 `1.1.4-beta3`；没有代码重构、新构建、卸载 MM、发布或合并。随后按用户要求只读完成了 SIM-03 自然续期验收，见第 4.11 节；旧卡/IPv6 问题仍待验收。

## 7. 最新 CI 与可用产物

| 项目 | 结果 |
| --- | --- |
| Validate Beta Refactor | [34681227857](https://github.com/autisticryptic/SimMaster/actions/runs/34681227857)，success |
| Build-Release | [34681227851](https://github.com/autisticryptic/SimMaster/actions/runs/34681227851)，success，含回归、arm64、amd64 |
| 候选源码 | `684e2a71e0227dc66f9b7591ab982485933bde15` |
| 包 metadata | `1.1.4-beta3` / `684e2a7` / `aarch64-unknown-linux-musl` |
| build_time | `2026-09-12T15:44:02+08:00` |
| artifact | `pkg-arm64`，ID `10293904588` |
| 发布状态 | 开发 artifact，Publish Release skipped，没有发布 beta4 |
| 本地目录 | `.codex-cf-candidates/684e2a7/` |
| 包 | `simadmin-candidate-684e2a7.tar.gz`，7,968,981 bytes，已部署 |

```text
pkg-arm64.zip SHA256:
5118e918131532c08dc19040ae1b6c38850646011a05fe3a3c5e539ab40cf467

simadmin-candidate-684e2a7.tar.gz SHA256:
e2a07b6e6c9d3c317d60549480bdfc34c6d2fda855a3e462ba5dfb67a2fcb496

包内 simadmin SHA256:
18408e9b2c60815a70d48d1a44dbcf85bb77bd0608357769a22b89acae58c167
```

同目录 `artifact.json`、`verified.json` 记录来源和校验。包只含程序、meta、www、devices，
不包含应覆盖用户的配置/数据库。下载端及部署端均已核验，实际架构为 AArch64。
公开 GET 下载地址为 [nightly.link](https://nightly.link/autisticryptic/SimMaster/actions/runs/34681227851/pkg-arm64.zip)。
artifact 的到期时间为 `2026-09-15T07:44:03Z`（北京时间 9/15 15:44:03）；异地续接可安全转移已校验包。
旧 aabe4e5 / 6baba57 的产物留作追溯，不能因版本字符串同为 beta3 就重新安装它们。

**发布门禁仍然有效：**开发分支 push 只产出 artifact；手动 `workflow_dispatch` 不保证“不发布”，
必须先审核门禁。不要把仍标 beta3 的修复先推到 master，再另行升版，以免覆盖 beta3 资产。
纯文档收尾若推送会触发发布的分支，应使用 `[skip ci]`。beta4 需在实机验收和必要回归后发布，
保持 pre-release、非 latest。

## 8. 设备最后状态与连接注意

> 以下为 21:06–21:10 的只读快照；自然续期已通过，观察器已结束，IMS 连接仍保留。详见第 4.11 节，不要沿用下午的 PID/Bearer 或重新等待已完成的续期。

### 8.1 设备身份和安装位置

- 入口：`https://qca410ssh.davidden.com/`，实际经授权 Cloudflare WebSocket 承载 SSH；不是直连公网 22。
- 系统：Debian 11、aarch64、`5.15.0-handsomekernel+`，管理默认路由 `wlan0`。不是旧局域网设备或旧文档中的 6.17 内核实例。
- 程序：`/opt/simadmin`，服务 `simadmin.service`；配置 `config.yaml`、数据库 `data.db`；服务使用 `SIMADMIN_CONFIG=/opt/simadmin/config.yaml`。
- 16:02:14 已成功安装 `684e2a7`，版本仍为 beta3；此前是 `6baba57`。
- 最近程序备份：`/opt/simadmin-backups/20260912-before-684e2a7-attempt1`。生命周期测试前的配置/数据库一致性备份另在 `20260912-before-mm-lifecycle-tests`；所有更早备份也保留。
- 初始旧安装完整封存：`/opt/simadmin-backups/20260908-214944-before-repo-beta2`，含 `old-app/` 和重置前配置快照。后续只替换程序，不重做干净配置，不覆盖这些备份。
- 晚间主程序 PID=`443`/NRestarts=0，ModemManager `433`/NRestarts=0、DATA6 initializer `662`/NRestarts=1、qmi-proxy `655`；这些值在本次自然续期前后不变。DATA6 的一次重启发生在 19:11 启动期基带复位后，不是续期期间重启。执行前仍须重新发现，不能硬编码。
- 当前 modem 对象 `/Modem/2`、唯一 `/Bearer/3`；不得沿用旧 `/Modem/56`、Bearer/54、接口或 PID。
- 线路标识 `line-50ad5391cd09c09936f1081bd479139c`，UE namespace `sa-ue286e0c9d2870`。换卡/重建后也应重新核实归属。

### 8.2 配置快照及本轮恢复

| 字段 / 项目 | 最后记录 | 续接要求 |
| --- | --- | --- |
| `volte_connection_enabled` | `true` | 保留当前连接，下一项实机测试前先确认卡、通话和配置，不无故重启 |
| 注册状态 | `registered=true`，IPv4/UDP，derived 首槽 | session_started_at=19:13:22，registered_at=19:14:13，reconnect=1；21:04:14 自然续期通过，refresh=1 |
| VoWiFi / 普通数据 / Trunk | 均关闭 | 不为试验擅自启用 |
| `trunk.vowifi_only` | `true` | 保留；Trunk 本身仍关闭 |
| `sms_path.force_vowifi_send` | `false` | **不能声称短信仅 VoWiFi 资费保护已开启**，不自动发短信 |
| `roaming_allowed` | `true` | 不等于漫游免费 |
| profile 来源顺序 | derived → carrier_catalog → database | 先保留，具体覆盖参数读 API 后再判断 |
| IP family 顺序 | ipv4v6 → ipv6 → ipv4，auto=false | SIM-03 有 IPv4-only 证据，但不要未经分析全局改设置 |
| `ims_video.volte_enabled` | `true`，随 IMS 连接恢复 | 此字段是连接状态镜像，不等于实际视频业务已通过 |

已核对 `LineProfileConfig::sync_ims_video_access_gates`：连接 setter 会同步 IMS video access gate。因此不能笼统说“关闭连接后其余所有字段都不变”，也不应另加视频开关来恢复。恢复连接后检查镜像、业务能力和费用字段。

### 8.3 跨环境连接

本地 `.codex-cfssh-client.py` 使用原生 Python 的 `paramiko`、`websocket-client`，通过 WSS 建立 SSH；凭据只从环境读取：`SIMADMIN_CF_AUTH`、`SIMADMIN_CF_SSH_PASSWORD`、`SIMADMIN_CF_WEB_PASSWORD`。三者分别涉及 Cloudflare、SSH 和应用认证，不是同一个密码。

这些 `.codex-*` 文件被 Git 忽略，普通 clone **不会带过去**。依赖包中的 Windows `.pyd` 不能直接用于 Linux。已知设备缺少 Python 3、jq、sqlite3，远端诊断应优先复用现有工具，不自动安装系统包或替换 libqmi。

`.codex-cf-resume-session.py` 依赖原 Windows 用户目录的历史会话路径，不能假定新环境可用。新对话不要批量打印旧会话寻找密码；需要时通过安全渠道恢复有效凭据。保留并核对 SSH host key，不静默接受变化。不要重放 `.codex-cf-commands.jsonl` 或旧部署脚本，不自动调用客户端的改密码功能。

`.git/codex_push_key` 本轮仅检查仍存在，没有读取或修改内容。历史 Git SSH 配置有 Windows 路径，换到 Linux 后需另行核对；可推送代码的 SSH key 不等于 Actions artifact API 凭据。不要把私钥或有效 cookie 一并写入交接文档/公开仓库。旧终端 session ID 也不能作为新对话可复用的连接句柄。

### 8.4 已核对的 API / 辅助客户端约定

以下是接口索引，不是要求新对话立即执行写操作。先完成身份、无通话、备份和候选校验：

| 请求 | 用途 / 注意 |
| --- | --- |
| `GET /api/cellular-ims/lines` | 响应外层成功为 `status="ok"`；线路 ID 在 `data[].modem.line_id`，配置及运行态分别在 `profile` / `runtime` |
| `GET /api/modem/lines/{line_id}/calls` | 线路通话列表；旧探测的 `/api/calls` 返回 404，不能拿它证明无通话 |
| `GET /api/modem/lines/{line_id}/cellular-ims/call/status` | 蜂窝 IMS 通话状态；部署前与原生通话检查一起核对 |
| `POST /api/cellular-ims/lines/{line_id}/connection` | 请求体 `{"enabled": true}` 恢复原连接意愿；`false` 是持久化关闭，不只是暂停一次 retry |
| `POST /api/cellular-ims/lines/{line_id}/retry` | 仅在确有需要时启动一轮恢复；202 表示受理，不等于注册成功。409 需读原因，可能已在运行、已注册或开关关闭，不应反复重试 |
| `PUT /api/cellular-ims/lines/{line_id}/profile-selection` | 三槽位配置；本轮不需要为了部署而改它 |

Rust 内部已采用 `CellularIms*` / `ImsProfile*` 命名，旧 HTTP 别名和部分序列化 `volte_*` 字段有兼容契约，不能全局替换字符串。操作后同时检查 HTTP 状态、API 的 `status` 和实际运行态，不把 HTTP 200 自动视为操作成功。

辅助客户端的 `run` 操作用 `path` 字段，不是 `script`；历史误用只产生 `KeyError`。客户端遇到操作错误可能继续读下一条输入，故不能把“校验、上传、部署、retry”无条件串成可重放的长队列。

## 9. 接下来按此顺序推进

**SIM-03 的晚间原会话自然续期已通过，见第 4.11 节，不需要再等待或重做该项。** 保留当前连接，下一步协调 SIM-01/02 在新 MM 路径上的回归，优先保全 45400 / IPv6 的地址就绪、路由和 modem 事件时序；Pixel 呼入单独受控测试。旧卡/业务未通过前不合并、升 beta4 或大规模切换后端。以下完整流程保留供回溯，不是需要无条件重放的操作清单。

1. **只读重建当前基线。** 核对仓库 HEAD、设备/内核、当前卡、运行 commit/哈希、配置、ModemManager 对象、主/次 QMI、网卡和 namespace、服务 PID。确认应用和原生活动通话均为 0；检查管理路由和 Trunk。记录与第 8 节的差异，别直接套用历史 PID。
2. **准备已验证候选（当前为 `684e2a7`，已部署）。** 如后续确需升级，使用对应新提交的 artifact 和独立 staging/backup，重新核验三层摘要；不要重放任何旧候选的一次性部署脚本。
3. **备份后部署，保护配置与依赖服务。** 停应用后进行一致性配置/数据库备份，包含可能存在的 WAL/SHM；仅替换程序资产。保留 ModemManager、proxy、DATA6 initializer 和管理链路，比较前后配置摘要/PID。失败回滚程序资产，保留配置与证据。健康检查除版本外还要核实 commit/哈希。
4. **恢复临时关闭的 IMS 连接。** 确认正确线路仍是目标卡后，通过已有接口恢复原连接意愿；复核 video 镜像同步、VoWiFi/普通数据/Trunk/短信资费字段未被意外改写。
5. **验证实际承载到 SIP 的完整链路。** 当前实现应核对 MM/proxy 持有的私有 bearer、D-Bus unique owner 和归属记录，不要求已被替换的 qmicli follow 进程存在。核对实际地址族、CGCONTRDP、对应数据网卡进入正确 UE namespace、地址/路由/P-CSCF，再观察 REGISTER → challenge/AKA → 成功响应。承载退出就停止使用旧地址，不在失效承载上重复试。
6. **验证该候选自然续期（SIM-03 首次已完成）。** 对新的卡/配置组合按网络租期正常调度，不缩成 120 秒。记录同一会话/transport、CSeq、refresh_count、租期、重连计数、owner/进程和接口；IPsec 路径还需原 SA 证据。探针成功或重新初始注册都不算此项通过，不重复等待第 4.11 节已经通过的测试。
7. **补回归和发布。** SIM-03 通过后，协调用户换回 SIM-01/02，验证最新主 QMI 实现无回退，并复测 SIM-02 catalog AKA 修复。记录库专属/派生路径区别；双注册和实际业务保留独立结论。满足修复验收条件后，先在修复分支统一 VERSION/Cargo.toml/Cargo.lock/前端版本为 beta4，补齐 `docs/releases/1.1.4-beta4.md`，经候选 CI 再合并并推送 master、完成 beta4 发布构建；不要先推仍为 beta3 的 master。通过发布包自身校验/验收后，再清理确认已合并的本地及远端开发分支。

若第 5 步仍失败，按层记录“proxy/owner → AT context → 网卡/namespace/路由 → P-CSCF 可达 → SIP 响应/AKA”，只输出必要脱敏摘要。不要在没有新证据时再次归因运营商，也不要无限自动重试。需要换卡、真实资费业务或影响管理链路的动作时再向用户确认。

本轮自然观察器的本地 exec session 是 `2407`，仅当前会话可用，不能跨环境复用。证据落在
`.codex-cfssh-results/ims-observation.*.json` / `natural-refresh-verdict.*.json`。
读取时必须匹配最终会话、SIM-03/derived profile 和当前 commit；不要把 SIM-01 的历史 verdict
或前两次重启后的初始成功当作本轮 refresh。若管理 SSH 中断，只读重连核对运行态和 journal，
不要先重启服务；若读取时已自然续期，应验证已有证据，而非再等一个完整租期。

后续每轮至少追加：时间/时区、commit/二进制哈希、SIM 脱敏标识及归属/服务网、requested/effective profile、实际地址族/P-CSCF、owner/接口/namespace、SIP 阶段、初始注册与自然续期的独立结论、配置差异、备份路径和下一步。失败记录保留，不以最后一次成功覆盖前面的归因修正。

### 注册修复后的独立语音问题：iOS 正常、Pixel 呼入进语音信箱

用户补充确认，之前的接打电话测试中，iOS 数据库可正常接打；Pixel 数据库虽然可以注册 IMS，但来电直接进入语音信箱，设备端无法接听。暂未固定当时的 SIM、版本、接入腿与实际生效 profile，因此不擅自把这条历史业务观察绑定为本轮 SIM-03 的实测。

后续固定同卡、同版本、同接入、同资费/Trunk 策略对照，至少区分：

1. **实际注册参数差异**：requested/effective profile、是否回退 derived、Contact 的 MMTEL/音频能力标签、注册身份与绑定、安全协商、Service-Route、租期/续期；不能只比较数据库文件名。
2. **设备是否收到来电 INVITE**：若没有，先查注册绑定、能力声明、网络侧呼入路由/转移；不能直接认定本地 RTP 或接听按钮有问题。
3. **收到后如何处理**：记录脱敏 SIP 状态/拒绝原因、VoLTE/VoWiFi 接收路径、Trunk/终端可用性、费用过滤及 SDP；收到 INVITE 但本地拒绝与网络直接转语音信箱是不同层级。
4. **业务验收授权**：当前不自动拨号、发短信或启用 Trunk。完成注册修复后协调用户进行受控呼入；注册成功、呼入响铃、接听接通、双向音频和自然续期分别记结果。

## 10. 源码与证据索引

### 仓库内可移植资料

| 位置 | 重点 |
| --- | --- |
| [QCA410 IMS bearer](../backend/src/hardware/devices/qcm410/ims_bearer.rs) | 已部署 `684e2a7` 的 `PrimaryImsRequest` 接入、地址族过滤、netdev 所有权；旧 qmicli flags/pipe 实现应从 `aabe4e5` 查看 |
| [MM 主 IMS session](../backend/src/hardware/devices/qcm410/primary_ims_session.rs) | 私有 bearer 创建、数字 selector 规范化、主口/接口/占用检查、取消和状态观察 |
| [MM 生命周期与归属](../backend/src/hardware/devices/qcm410/primary_ims_lifecycle.rs) | unique-owner D-Bus 调用、原子归属账本、namespace/config guard、shutdown 和重启恢复 |
| [netdev](../backend/src/hardware/devices/qcm410/netdev.rs)、[secondary_qmi](../backend/src/hardware/devices/qcm410/secondary_qmi.rs)、[secondary_qmi_data](../backend/src/hardware/devices/qcm410/secondary_qmi_data.rs) | 主/次端点、数据网卡解析和普通数据归属 |
| [transport](../backend/src/hardware/devices/transport.rs) | 多设备承载接口、所有权和存活检查边界 |
| [cellular_ims](../backend/src/connectivity/modems/ims/cellular_ims/) | `live.rs`、`pcscf.rs`、`native_bearer.rs`、`data_slot.rs`、`errors.rs`；注册/续期/轮询/承载失效 |
| [catalog v7](../backend/src/connectivity/modems/ims/vowifi/carrier_catalog_v7.rs)、[profile_store](../backend/src/connectivity/modems/ims/vowifi/profile_store.rs) | 初始认证默认值、可选库和 source-bound 引用 |
| [config](../backend/src/platform/config.rs)、[API handlers](../backend/src/api/handlers.rs) | 连接开关兼容字段、video 镜像与已有 API |
| [HTTP 路由](../backend/src/main.rs) | canonical / legacy API 对照；诊断前核对真实路径、方法和返回结构 |
| [DATA6 service](../deploy/devices/qcm410/system/simadmin-secondary-qmi.service) | 项目创建 DATA6 的职责说明 |
| [beta validation](../.github/workflows/beta-validation.yml)、[build release](../.github/workflows/build-release.yml) | Actions 编译、回归、构建与发布门禁 |
| [release policy](../.github/scripts/release_version.py) | beta 预发布和 latest 判定 |
| [DNS 迁移](DNS_HICKORY.md)、[IMS 命名迁移](IMS_NAMING_MIGRATION.md)、[接入共存](IMS_ACCESS_COEXISTENCE.md) | 前期重构和兼容约束；不是本次最新候选的业务验收 |

**旧文档要按时间和设备解释：**

- [plan.md](../plan.md) 保留历史阶段和 beta3 发布验收，但开头“当前 beta2”的摘要已过期。
- [DEVELOPMENT_PLAN.md](DEVELOPMENT_PLAN.md) 是长期待办入口，现已补充 1.1.5/1.1.6 路线并修正 QCM410 主 IMS / DATA6 普通数据描述。其余沿用 8/30 的测试数量、其它设备通话结果及“完全没有实现”的判断，仍需核对后才能当作新版本状态。
- [ENVIRONMENT.md](ENVIRONMENT.md) 和 [QCM410_BAM_DMUX_MODEM_CRASH.md](QCM410_BAM_DMUX_MODEM_CRASH.md) 的历史设备/secondary IMS 描述不能覆盖当前主 QMI 设备契约；后者也不能证明这台远程设备发生了相同内核故障。
- 旧 README/笔记里的 `connectivity/modems/softstack/...` 不再是本轮源码定位入口；当前相关实现见 `connectivity/modems/ims/{cellular_ims,vowifi}`。
- 双注册及各自续期、多线路/多硬件矩阵、真实短信语音/视频、Ut/MWI/E911、VoNR 等仍要分别验收，不能以这次 QCA410 注册修复代替整个项目完成。本轮不批量重写旧文档或扩大产品改动。

### 本机证据，不随普通 clone 携带

- `.codex-device-notes/qca410-ims-2026-09-08.md`：逐卡逐轮台账，S03-T24 记录晚间自然续期；旧暂停点不能替代最新结果。
- `.codex-cfssh-results/`：脱敏设备观测；最新 SIM-03 自然续期结论与证据索引见第 4.11 节的 verified JSON，不能拿历史 api-safe 文件当当前状态。
- `.codex-cf-candidates/684e2a7/`：当前已部署候选包与校验清单；更早目录仅用于追溯。
- `.codex-cfssh-client.py`、`.codex-cf-fetch-candidate.py`：连接/传输和 artifact 下载校验辅助工具；下载脚本默认参数是旧候选，不能无参数运行。
- `.codex-cf-deploy-candidate-4a5d3b2-attempt2.sh`：只能审阅保护措施，不能原样重放。
- `.codex-cf-sim03-mm-reference-probe.sh`、`.codex-cf-sim03-mm-namespace-probe.sh` 等本轮探针的 `.codex-cfssh-results/` 结果：MM 参考与 namespace 对照证据。脚本不能原样重放。
- 原始会话 1：`/root/.codex/sessions/2026/09/09/rollout-2026-09-09T13-10-04-01a08492-bcfe-70f1-b528-4ae69bbe5d18.jsonl`。
- 原始会话 2：`/root/.codex/sessions/2026/09/12/rollout-2026-09-12T00-19-38-01a09144-76f7-7c52-acfb-618326ae5131.jsonl`。

若追溯原会话，注意部分 assistant 消息的 `phase` 为 `final_answer`，后台命令完成结果在 `event_msg.payload.item` 的 `CommandExecution` 中；只搜最后一条文本回复会漏掉已完成的下载。以下行号对应本轮读取的原文件：

| 原始记录 | 关键证据 |
| --- | --- |
| 会话 1，L319、L1673、L2385 | 验收后合并/升 beta4/清理分支；保留 key；构建交给 Actions |
| 会话 1，L3102–L3627 | 用户提供旧版可用对照，并反复确认 QCA410 主 QMI / DATA6 职责、禁止增加用户模式开关 |
| 会话 2，L505 | 旧 qmicli 掩码覆盖的发现，以及有界探针结论 |
| 会话 2，L585 | **两条 CI success、候选下载/校验完成、release_published=false** 的命令实际输出 |
| 会话 2，L596 的交互结果 | 4a5d3b2 部署、临时关闭 IMS、probe、00:40 配置/承载快照；此处进程被中止不等于所有内部命令失败 |

原会话可能含凭据，禁止整份公开或粘贴到新对话。本文不包含密码、token、私钥、完整用户身份或认证报文，也不能代替新环境所需的安全凭据。

### 异地续接的最小材料

1. 本文、`MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md`，以及包含 `684e2a7` 程序代码和文档提交的修复分支；不要只克隆停在 `48612e2` 的 master 就开始改代码。
2. 若准备部署，携带第 7 节候选包及 `verified.json` / `artifact.json`，或在到期前下载并核对摘要。
3. 如需沿用诊断脚本，单独安全转移并审阅 `.codex-cfssh-client.py` 等被忽略文件；需要追溯时再携带脱敏台账，不要求重放整份会话。
4. 独立提供有效连接凭据和已核验 host key。若缺失，仅该远程步骤受限，本地源码审阅仍可进行；不要继续把“旧 artifact API 401”当作当前必然阻塞。

## 11. 新对话启动文本

```text
请先阅读 docs/PROJECT_HANDOFF_2026-09-12.md，再核对当前工作区和设备状态，
在此基础上继续 SimAdmin，不要重放历史命令。

分支 fix/sim02-catalog-aka-baseline，设备程序代码为 684e2a7，版本仍为 beta3；
文档提交可能在该提交之上，先核对 git HEAD，不要把文档提交当作新部署程序。
两条 CI、双架构构建、受保护部署均通过，SIM-03 完整认证、初始注册和首次自然续期通过。
正常关闭清理/自动重新注册、主进程 SIGKILL 后遗留承载回收/自动注册也通过。
18:48 曾遇 Cloudflare 1033，19:19 已只读重连；用户确认当前为 SIM-03，
设备/SIM 读取和日志均为 45403 / 46000。19:14:13 已自动重新完成认证和注册。
当前 session_started_at=19:13:22（UTC 11:13:22），registered_at=19:14:13，
derived 首槽，IPv4/UDP、Bearer/3；21:04:14 原会话续期成功，reconnect=1、refresh=1。
48 个只读样本和 journal 已核对，CSeq=4，续期新租期7200秒/下次间隔6600秒。
原 UDP tuple、MainPID443/FD19/socket inode12411、UE namespace inode4026532440不变。
观察器已正常结束，设备 IMS 连接保留；不要为重复验收而重启、retry 或缩短租期。
主程序 PID=443，MM=433，proxy=655，DATA6=662；ledger 1 个，UE namespace 保留。
只读 reboot-investigation 已执行：19:11:56 基带 intentional reset 后恢复，
19:13 自动 recovery 做了端口重新通知，未重启 MM；早期墙钟日志跳变，勿凭其归因。
下午 16:39 会话的续期观察已中断；45400 / IPv6 路由失败与 modem 消失仍独立待查，
不要把当前 SIM-03 初始注册当作旧会话续期，或当作旧卡/IPv6 回归通过。
先读第 4.11 节核对已完成的自然续期证据，不必重新开始两小时等待。
下一步协调 SIM-01/02 在新 MM 路径上的回归，并独立排查 Pixel 呼入问题。
后续新卡测试和旧卡复测均按第12节追加，插入文末固定尾注的定位标记之前；
固定尾注必须保持在文件最后，失败/中断记录不覆盖，第1/5节仅同步最新摘要。
用户已确定 1.1.5 双后端兼容、1.1.6 完全原生接管，详细规划见
docs/MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md 和第 6.6 节。
只写了规划，没有实施重构、修改版本号或授权现在直接卸载设备上的 MM。
初始注册、自然续期、双注册及业务测试分别报告，不能改短租期来凑结果。
另有 iOS 可接打、Pixel 注册后呼入进语音信箱的问题，需要单独业务对照。

QCA410 固定主 QMI + qmi-proxy 做 IMS，项目创建的 DATA6 只做普通数据；
不要新增用户接口选择开关，不要影响其它硬件，不要本地 Rust 编译。
保留配置、数据库、备份、证据、依赖服务和 push key，不自动拨号/发短信/改费用设置。
SIM-01/02 在正式 beta3 上成功不等于最新架构已通过。
实机修复及必要回归通过后再合并 master、升 1.1.4-beta4、清理已合并分支；
保持预发布、非 latest。每一步区分代码完成、CI 通过、设备实测和未验证项。
开发分支 push 的 artifacts-only 行为不代表手动 workflow_dispatch 也不会发布。
本文与版本规划应随修复分支提交 GitHub；.codex-* 辅助文件仍需单独安全携带，
不能重放旧部署/测试脚本或把原始凭据会话公开。
```

## 12. 后续多卡测试记录（持续追加）

SIM-01～SIM-03 的既有历史与验收保留在第 4、5 节，不复制成新的测试结果。
从现在起，**所有新增卡测试及已有卡的复测，都按时间顺序追加在本节末尾、下方固定尾注之前**。
新卡使用不重复的匿名编号（SIM-04、SIM-05……）；同卡复测另开轮次，不能用新结果覆盖旧失败。

本节是后续测试明细的统一追加区；第 1 节和第 5 节只同步结论摘要及对应记录编号。
记录格式与验收边界见固定尾注中的模板。尚未执行的测试只写计划或“未测”，不预填通过结果。

<!-- SIM_CARD_TESTS_APPEND_BEFORE_NOTE -->

## 固定尾注：为什么需要多卡回归，以及后续记录放在哪里

> **位置约定：本尾注始终保留在文档最后。所有后续卡片测试记录必须插入上方定位标记之前，不得追加在本尾注之后，也不要在下方模板内直接填写实测结果。**
>
> 定位标记名称：`SIM_CARD_TESTS_APPEND_BEFORE_NOTE`。先复制模板到标记前，再填写；不要移动或删除标记和尾注。

### 回归测试的目的

回归不是怀疑某张卡能不能注册，而是确认：**修好一张卡，没有把原来能用的功能修坏。**

- 本轮优先安排 SIM-01，是因为它在旧版有 IPv6 注册和自然续期成功的基线；SIM-03 此次通过的是 IPv4，不能替代 IPv6 验证。
- 此前现场出现过 IPv6 路由错误，因此需要对照检查；这不等于已经认定 SIM-01 被修坏。
- 共用承载、鉴权、profile、网络隔离或恢复逻辑变化后，需要不同卡、运营商、地址族和配置的独立证据。一张卡成功不能推导其它卡全部成功。

以上是选择对照卡的背景说明，**不是永远待执行的任务清单**。某项是否已通过，以最新测试记录及第 5 节矩阵为准，不重复等待已经完成的同一会话续期。

### 长期记录规则

1. 用“卡别名 / 日期 / 测试轮次”唯一标识记录，例如 `SIM-04 / 2026-09-13 / T01`；同 PLMN 的不同卡仍分别编号。
2. 每轮明确版本、设备、卡、网络、后端、requested/effective profile 和实际地址族；对照时尽量只改变一个变量。
3. **初始注册、自然续期、呼入、呼出、双向音频、短信和双注册分别记状态。** 未测、失败、中断和不适用不能写成通过；不适用需说明依据。
4. 续期必须核对原会话/通道及重连计数；重启或重建后的初始注册不算自然续期，不缩短网络租期来凑结果。
5. 保留失败、原始判断及后续修正的时间线；复测新增条目并引用旧记录，不覆盖历史来制造“全部成功”。
6. 仅写匿名卡标签和脱敏证据，不写完整 IMSI/ICCID/号码、密码、Cookie、私钥或 AKA 材料。通话/短信测试仍须获得授权，不因新增测试条目自动执行。

### 记录模板（复制到尾注前使用）

```markdown
### SIM-XX / YYYY-MM-DD / TNN — 本轮测试目标

- 时间与时区：
- 卡别名 / 归属 PLMN / 访问 PLMN：
- 设备 / 固件 / 内核：
- 线路 ID / UIM 槽位（适用时）：
- 程序版本 / 代码 commit / 二进制摘要：
- 后端 / 控制口 / 数据接口：
- requested profile / effective profile / 数据库或 catalog 版本：
- 实际地址族 / SIP transport：
- 本轮变量与前次记录的关联：
- 初始注册 / 认证：未测（填写状态、时间与证据）
- 自然续期：未测（时间、租期、CSeq、refresh/reconnect 计数、原会话/通道证据）
- UE 隔离 / 承载归属 / 资源清理：未测
- 呼入 / 呼出 / 双向音频 / 短信 / 双注册：分别标记，默认未测
- 错误与复现条件：
- 脱敏日志、观测文件或相关提交：
- 本轮结论 / 未覆盖范围 / 下一步：
```
