# SimMaster 单主分支整理（2026-09-24）

用户要求检查多余分支，在不丢失有用改动的前提下只保留主分支。
真实远端是 `autisticryptic/SimMaster`（本地名 `simmaster`），默认分支为 **master**；
本地 `origin` 指向旧工作区，不是此次 GitHub 清理目标。

## 1. 整理前的事实

| 分支 | 检查点 | 相对 master 的状态 |
|---|---|---|
| master | `48612e2` | 原主分支 |
| refactor/1.1.4-beta2 | `48e37fc` | 已完全合入 master，主分支领先 6 提交 |
| fix/1.1.4-beta3-cellular-ims | `07ef05c` | 已完全合入 master，主分支领先 2 提交 |
| fix/sim02-catalog-aka-baseline | `4c1a738` | 有 20 个提交未进 master，但全被 dev 包含 |
| dev/1.1.5-modem-backends | `2129282`（本轮 native 增强后为 `efe6135`） | 分别领先 master 65 / 66 提交，无分叉 |

因此不能直接把所有非主分支删掉：应先把 dev 上的完整历史快进整合到 master，
再删除已被 master 覆盖的分支引用，不 squash、不重复 cherry-pick、不重写历史。
整理前无打开 PR，未启用分支保护；操作仍逐项核对远端 SHA，防止覆盖其他人的新提交。

## 2. 必须先处理的发布风险

此前 `release_publication.py` 允许 **master push 自动发布**，当前 VERSION 却仍是
`1.1.4-beta3`。已有同名 Release，且发布 action 使用 `overwrite_files: true`。
直接推进 master 会有覆盖旧制品、让 tag 与包内 commit 不对应的风险。

本轮在任何 master 推送之前修改：

- 所有普通 push 只构建 Actions artifacts；仅 master 上显式 `workflow_dispatch`
  且 `publish_release=true` 才能发布。
- release job 自身再次检查 event/ref/output，写权限仍只授给该 job。
- 发布前拒绝已存在的 tag 或 Release；404 才表示不存在，认证、限流、重定向、服务器错误
  均 fail closed。同版本发布串行，资产禁止覆盖。
- master 和未来临时 `dev/**` 执行同样的回归；master PR 接入 beta-validation。
- 前端 lint 加入 Build 的 prepare gate，不能在 lint 失败时发布。
- 不启用当前已手动停用的 LPAC 发布工作流，不改应用版本，不创建/覆盖标签或 Release。

## 3. 执行验收

- [x] 核实所有分支祖先关系及唯一历史，保存远端分支/Release/tag 的整理前只读快照
- [x] 补普通 push 不发布、显式发布拒绝复用 tag/Release 的测试与工作流保护
- [ ] 开发分支新保护与 native 增强的 CI/双架构通过，发布 skipped
- [ ] master 快进到包含全部成果及保护的提交，master CI 通过且发布 skipped
- [ ] 对比旧 Release 资产/元数据与 beta3 tag，确认未被本次整理改写
- [ ] 删除四个已合并远端分支，并确认 GitHub 只剩 master
- [ ] 清理本地已合并分支；保留工作区目录及未跟踪文件，不删除用户日志/脚本/证据

本地工作区计划：主目录 `SimAdmin` 使用 master；保留 `SimAdmin-1.1.5` 为 detached
快照工作区，避免为删除分支而删除目录。原始会话、私密凭据、下载包不加入提交。

## 4. 能力与发布不是同一件事

合并主分支只表示保留与整合代码，不表示实验性 native 后端已取得端到端实机验收。
默认 MM、显式 native opt-in、独占 owner 与未确认 receipt 保护都继续保留。
N2 完整事件业务层、Quectel/DJI 专用写入、APDU 通道账本、混合 owner 和 native 硬件验收
仍按各自计划执行，不能因收口分支而宣称已发布 1.1.5/1.1.6。

标签独立保留；本地与远端曾存在不同历史 tag，不使用 `push --tags`、`--mirror` 或强制同步标签。
