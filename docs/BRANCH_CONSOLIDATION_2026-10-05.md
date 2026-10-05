# 单主分支维护与本次整合

## 完成结果

2026-10-05已整合并推送 **`ae3926e37a285770aa802f74def33c1b3d491f98`**，保留9个父提交，
包含所有旧验证快照历史。主分支[Build37276695025](https://github.com/autisticryptic/SimMaster/actions/runs/37276695025)、
[Validate37276695143](https://github.com/autisticryptic/SimMaster/actions/runs/37276695143)、
[Frontend37276695033](https://github.com/autisticryptic/SimMaster/actions/runs/37276695033)全部success。
两套实际日志68项相关回归与54个矩阵场景通过，双架构包完整校验，Publish Release skipped。

07:39 UTC复核：**本地及GitHub仅保留master**，下列五条dev分支已原子删除；
8条本地临时build-snapshot引用也在确认属于master祖先且bundle备份有效后清理。
`v1.1.5`标签仍指向原5c378f8，旧Release未覆盖。随后文档收尾提交不改变构建输入。

## 维护方式

当前项目以 **`master`** 为唯一日常开发分支，GitHub远端为 `autisticryptic/SimMaster`（本地remote名`simmaster`）。
先前为隔离未提交工作、运行GitHub Actions创建的临时验证分支，在主分支整合并通过验证后删除。
不继续把每次验证快照长期保留为GitHub分支。

Release标签与分支不同：`v1.1.5`保持原指向，不因主分支前进覆盖旧发布。
本地私有构建快照/归档也不是日常开发分支；不得把备份当成待发布代码。

## 整合的内容

最终生产代码选择已验证的 `6ddc75157a106513aa90f80b7b4b0b15d66251a1`：包含安装/eSIM页面、
默认地址族兜底、换卡清理屏障、跨owner恢复及CMCC 421定向候选等此前工作。
旧快照不是五套独立功能分支，而是从同一5c378f8创建的累积工作树快照；冲突以已验证后继源码解决，
不把旧版本覆盖回最新修复。通过合并提交保留原提交历史，而不是只删除分支指针丢弃独有提交。

已删除且提交历史仍由master保留的五条临时远端分支：

- `dev/ims-switch-20261004T120258Z-2785dbf6`
- `dev/ims-reconcile-20261004T152003Z-395c209e`
- `dev/ims-reconcile-20261004T154159Z-c60d5e12`
- `dev/ims-cmcc-20261005T050010Z-67a611ec`
- `dev/ims-cmcc-20261005T053527Z-d295ae33`

## 清理门槛与恢复依据

1. 先记录所有分支/标签/构建快照和工作树指纹，生成并验证Git bundle备份。
2. 整合后的构建输入与6ddc751逐项核验；文档可更新，但不偷偷混入新的行为改动。
3. 主分支使用GitHub Actions完整验证，所有push仍仅生成artifact，不自动发布Release或部署设备。
4. 逐条证明临时分支提交已是主分支祖先，再删除对应远端分支；若指针被其他人推进，拒绝删除。
5. 复核远端仅保留master、原Release标签未变、本地私有内容未被提交。

本次本地证据目录：`.local/evidence/branch-cleanup-20261005/`，包括
`backup-state.json`、`pre-consolidation.bundle`、整合提交/CI记录和最终分支核验。
该目录不随Git提交。bundle保存清理前引用，必要时可在另一个目录用`git clone`/`git fetch`读取恢复，
不要为了查看备份而重置当前工作树。

## 明确保留的工作区内容

用户原有的 `docs/archive/2026-09/ESIM_IMS_PROFILE_TEST_2026-09-01.md` 删除，以及
未跟踪 `docs/ESIM_IMS_PROFILE_TEST_2026-09-01.md` 的私有修改保持原样，不夹带提交。
`message.txt`、诊断日志、JSONL、设备资料也不入库。
因此“只保留master”不等于强行清空用户工作区或把所有未跟踪文件上传。

## 实网状态不随合并改变

本次是代码和分支管理，不操作410、不重启MM/基带、不安装新数据库。
CMCC候选仍需要同卡实网验证；主分支CI通过不能替代实网注册成功。
最近设备部署记录与候选说明仍以[HANDOFF](HANDOFF.md)中带日期的证据为准。
