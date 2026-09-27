# 当前接手与项目状态

> 更新：2026-09-27。**本文件是唯一当前接手入口**；历史记录在 [archive](archive/README.md)，
> 私有操作材料在本机 `.local/`。不要根据旧文档的“当前版本/下一步”重放操作。

## 1. 当前优先级

1. **版本/发布已完成**：源码、tag、发布包统一为 **1.1.5 / `16998ae`**，用户手动触发的
   Build-Release `36253740079` 已全绿，GitHub `/releases/latest` 已为 `v1.1.5`。
   两架构包实际下载后确认 SHA-256、包内版本/commit 与 ELF 架构一致，不是只改 Release 标题。
2. **SIM-06 中国电信 IMS 注册失败**：用户确认设备离线，暂停访问与轮询。
   用户另确认 beta8 与源码参考构建均可注册此卡；已核对源码参考的真实 Python daemon 入口，
   首轮机制对照见 [IMS 诊断 §7](IMS_DIAGNOSTICS.md#7-sim-06两套成功参考的生产链对照2026-09-27)。
   待上线先核实现场 backend/profile/失败阶段；静态差异不等于已定位或修复。
3. native 真机及其他长期任务按 [开发总计划](DEVELOPMENT_PLAN.md) 和
   [后端路线图](MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md) 单独推进，不并行抢占同一设备。

SIM-04 自然续期、SIM-05 手动测试已由用户确认完成，不重复等待或验收；
它们的 MM 结果不代表 native 全能力通过，也不扩大解释 SIM-05 的业务测试覆盖。

## 2. 完成边界：不能称为全项目彻底完成

| 范围 | 状态 |
|---|---|
| IMS 命名/数据库迁移、Hickory DNS、Telegram 反代入口 | 约定代码阶段已完成，有回归/历史 CI；保留旧值兼容与各自实机限制 |
| beta8 与朋友提供的三网源码 | 主要流程及可移植边界有归档分析；不是穷尽所有二进制分支或全部移植 |
| native AT/URC、持久化短信、分片、逐片送达、受控恢复 | 已有代码和 CI；最终功能检查点 `302b70e` |
| native 真机、Quectel/DJI 维护、长稳/代次故障 | 尚未验收，不由 MM 测试替代 |
| 同机不同 modem 混合 MM/native | 尚未实现；目前全局二选一 |
| 未知孤儿资源自动恢复、完全统一跨旧 IMS 的去重/通知恰好一次 | 不在已实现承诺内；未知 receipt 保持阻断 |
| 双注册、多线路、VoWiFi/视频、UT/MWI、E911、CS 音频、1.1.6 | 保留各自实现或实机/发布门槛，不能一并勾选完成 |
| SIM-06 | 没有该卡现场根因、修复或验收证据 |

详见 [原生后端当前状态](NATIVE_BACKEND_STATUS.md)。旧总计划存在日期较早的条目，
逐项以最新代码和证据核对，不机械地把所有旧复选框重开或清空。

## 3. 只保留一个开发目录

- 唯一工作区为 **`SimAdmin` / `master`**，GitHub remote 为 `simmaster`（`autisticryptic/SimMaster`）。
  `origin` 是历史本地 remote，不要误推；实际 HEAD 以 `git rev-parse HEAD` 为准。
- 整理前 HEAD：`19c3c122afdf6f053d02b1f1a9a2e3b26a7595bd`。
- 原 `SimAdmin-1.1.5` 不是较新代码，而是 `586985e` 的 detached 快照，落后 12 个提交，
  完整历史已在 master；确认无独有源码、只有 Python 缓存和依赖后已移除该 worktree。
- 目录名、版本字符串、发布标题、tag、二进制 commit 是不同概念。不要重建旧目录来“切到 1.1.5”。

## 4. 验证与发布证据

### 1.1.5 已核验发布基线

- 真实源码提交：`16998ae3c5172890075b2392ded3ce9c711d72b8`。
- push 构建 `36252976546`、Validate `36252976632`、Frontend `36252976689` 均 success；
  push 的 Publish Release skipped 是原门禁行为，不是构建失败。
- 用户手动 dispatch 构建 **`36253740079`** 全部 success，含前端、Rust 回归、arm64/amd64 及 Publish Release。
- Release **`397300521 / v1.1.5`** 为非 draft、非 prerelease、latest；tag 指向上述源码 SHA。
  用户删除了旧 Release；不能继续建议操作不存在的 v1.1.7/v1.1.8 Release，旧 tag 与 Release 分开看。
- 2026-09-27 实际下载验证：两个 `meta.json` 均为 `1.1.5 / 16998ae`，target 与 ELF 机器类型匹配。

| 包 | SHA-256（与发布的 SHA256SUMS 一致） |
|---|---|
| amd64 | `34920b6d0760bc8b16463a736c8e20513457c01e0c75ada0b39e043747abc92f` |
| arm64 | `eba1704d6b760142b8a724547781bb4baa40be5df90cd8b822cb0ed555a5d491` |

- 本地检查为 **169 项 Python 测试通过**，含 28 项采证回归；Rust 仅在 Actions 编译/测试。
- 可核验记录在 `.local/evidence/release-1.1.5/github-verified.json` 与 `artifacts-verified.json`。
- 后续 docs-only 提交可晚于发布 tag；不要把新的文档 HEAD 当作已发布二进制的 commit。

### 历史与未验收边界

`302b70e` 的旧 CI 和早期接手快照仍保留，但不再代替上述真实 1.1.5 证明。
本次发布核验和首轮静态对照（`c601a09`）未部署设备、未改变 IMS 注册算法、未改 MM 默认。
后继 Security-Server 列表修补见下节，不能与先前发布源码混同。
此前对其他提交号、已完成身份错误码补丁或“native 全部验收”的口述不能当证据；
以 Git、明确的 CI run、实际下载包和当前源码为准。

### 后继安全列表修补：代码/CI 已完成，尚未发布或部署

`dfda6cd2ed51e6ee450752a7b565cdb97d4b7906` 补齐多行/逗号 Security-Server 的候选边界与
完整 Security-Verify 回传，不改变默认客户端算法、MMTEL 或 MM 后端。
新增 17 项 Rust 测试及既有 refresh 接线回归，两套 Actions 已实际运行通过：

- [Validate `36291639402`](https://github.com/autisticryptic/SimMaster/actions/runs/36291639402)：success。
- [Build `36291639395`](https://github.com/autisticryptic/SimMaster/actions/runs/36291639395)：前端、
  Rust 回归、amd64/arm64 musl 编译及打包全部 success，Publish Release 按 push 门禁 skipped。
- 本地 172 项 Python 检查、定向 Rust 格式和 diff 检查通过；未在本地编译 Rust。
- 明细证据：`.local/evidence/sim06/security-agreement-ci.json`，对应提交与 job/step 结果已核对。

具体范围及保留的客户端单候选限制见 [IMS 诊断 §8](IMS_DIAGNOSTICS.md#8-后继候选修补security-server-列表2026-09-27)。
这不是 SIM-06 根因/实机修复结论，**已发布 v1.1.5 仍对应 `16998ae`，不包含此后继修补**。
不能重新对现有 tag 直接发布覆盖资产；若需发行或部署候选，应另行明确版本及目标。

## 5. 本地资料布局

| 路径 | 用途 |
|---|---|
| `.local/active/ims/connect_readonly.py` | 当前只读 Cloudflare/SSH 入口；其依赖及已保存主机公钥 pin 同目录 |
| `scripts/ims-readonly-evidence.sh` | 随源码维护的规范采证脚本 |
| `.github/scripts/test_ims_readonly_evidence.py` | 采证工具回归 |
| `.local/evidence/sim06/` | 脱敏访问记录，不是当前在线状态 |
| `.local/evidence/ci/` | 历史 CI 核验与后续构建证据 |
| `.local/checkpoints/pre-cleanup-2026-09-26/` | 整理前 15 文件快照、补丁和测试记录 |
| `.local/cleanup-2026-09-26/` | 原目录清单、52 份原文档、移动/删除清单 |
| `.local/archive/` | 原 `.codex-*`、`.tmp-*`、`.tmp/`、旧 release 包和会话；保留唯一数据及证据 |

`.local/` 不随 Git 分发，其中历史资料可能含凭据，不公开打包。普通 clone 不包含访问权限。
原根目录三份 carrier SQLite 数据保留原位置；主目录的可用前端依赖和构建资源也保留。
Git 私钥及仓库外私密交接未删除或覆盖。

需要原始用户原话时，才查 `.local/archive/sessions/2026-09-24T.jsonl`，更早参考
`2026-09-19.jsonl`；程序化定位并脱敏，不整段输出 Cookie 或原始工具结果。
历史脚本仅作证据，不批量执行、不自动重建旧 bundle、不恢复旧部署流程。

## 6. 设备上线后的只读第一轮

1. 用户确认上线后，先核对本工作区 Git/diff、规范脚本及 `.local/README.md`。
   审阅只读入口后使用既有 Python 环境；不要运行通用客户端的写操作主程序：

   ```sh
   /root/.cache/simadmin-mm-resume-venv/bin/python .local/active/ims/connect_readonly.py
   ```

2. 私密凭据和 host-key pin 缺失时由用户安全提供；不猜密码、不自动信任新主机、不回显秘密。
   最后旧观察 `2026-09-26T02:44:53Z` 为 HTTP 530 / Cloudflare 1033，未到 SSH。
   新错误重新分类，不能认定未来仍是离线或 Cookie 一定有效/过期；失败不连续轮询。
3. 先看 `/proc/<MainPID>/exe` 哈希、安装 metadata、服务 PID/start ticks/启动类型。
   采样期间进程稳定不证明历史 journal 全部来自该程序版本。
4. 经既有授权应用登录，另行只 GET `/api/cellular-ims/lines`、该线路详情、`/api/modem/backend`。
   核实实际 `line_id`、SIM 作用域和 MM/native 后端，不把“SIM-06”直接当 API line ID。
5. 获取 `phase/stage/last_error`、连接/候选尝试、恢复和下一重试时间及未解决 receipt。
   按 bearer/IP 族 → P-CSCF → AKA/安全协商 → 真实 SIP 响应定位，归属必须和线路/时段交叉核对。
   只读入口不会自动登录应用、查询这些 API 或认定日志属于 SIM-06。
6. 无证据不猜 APN/身份/运营商特例；明确缺陷后才改代码。部署需当次确认，核验目标、无通话、
   制品 commit/版本/架构/校验和；不根据目录名或 Release 显示名称部署。

采证字段、脱敏和输出边界见 [IMS 只读诊断](IMS_DIAGNOSTICS.md)。

## 7. 不得被旧摘要覆盖的约束

- MM 保持默认；native 必须显式实验 opt-in，不能自动停 MM、接管、写 NV/USB 或重启设备。
- 单一 agent 修改代码/配置/部署，其他 agent 只读；不自动拨号、发短信、改变费用保护。
- 用户取消了测试窗口自动回滚；不恢复固定 120 秒 refresh、旧包激活或续期轮询。
- 重插/重启/节点换代不等于资源消失；未知 receipt 不删除，已释放 CID 不重放。
- `a4a83c2` push 失败、SIM-04/05 等待验收等旧摘要已经过期。
- 持久化 `volte_ims` 已通过迁移归一到 `cellular_ims`，保留旧值读取；不要回退迁移，
  也不机械替换剩余历史名称或真实 VoLTE 语音语义，见 [命名与兼容](IMS_NAMING_MIGRATION.md)。
- Rust 编译/测试/双架构构建只在 Actions。本地允许 Python、格式/语法和文档检查。

## 8. 新对话可直接复制

> 请先读 `docs/HANDOFF.md`，核对当前 Git/diff 和版本修正/CI 状态。
> 唯一开发目录是 SimAdmin/master，旧 SimAdmin-1.1.5 已安全收口，不能当较新分支。
> 若我已确认设备上线，再按文档只读核实 SIM-06 中国电信的实际版本、线路和失败阶段；
> 否则不要连接或轮询设备。不要重做 SIM-04/05 验收，不清理 `.local/` 或私密材料，
> 不把旧 CI、安装 metadata 或历史日志当新程序实测。无现场证据不猜根因，部署需另行确认。

更多文档按 [文档导航](README.md) 查阅；新进展更新本文，不再另建根目录接手副本。
