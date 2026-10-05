# 安装、eSIM 页面与数据库变体交付（2026-10-02）

本轮续接 `2026-10-01T.jsonl` 最后未完成的三项需求。代码变更位于 SimAdmin 与相邻
`carrier_Bundles` 两个仓库的工作区；**没有提交、推送、发布、连接或部署到 410**。
10 月 1 日的 `448c98a` IMS 注册验收是历史记录，本轮没有据此声称设备实时状态。

## 安装修复

- 用 `scripts/package-release.py` 统一本地/CI 打包，包内包含离线安装器、同版本主 unit、
  设备资源、元数据与完整 SHA256 清单；避免旧包与新分支 unit 混装。
- `deploy/install.sh` 与当前工作目录无关；`install_latest.sh` 保留文件名但要求明确的
  可信 `REPO` / `VERSION`，不隐式选择 latest 或第三方镜像。
- 停服前完成完整性、ELF 架构、二进制版本、现有配置和服务设置预检；默认只安装文件。
- 不覆盖配置、数据库、catalog、lpac；显式 `--activate` 才允许主服务维护操作。
  失败时回滚拥有的文件和主 unit，并尝试恢复原服务状态。
- 已有服务关闭的设备/网络操作权限不能被升级改回开启；自定义 unit/drop-in 会被拒绝。

新格式包须重新构建，旧 Release 不会自动被修好。使用与回滚边界详见 [安装指南](INSTALL.md)。
设备资源 CLI 的某些失败仍只以文本报告，故未被塞进默认安装事务；硬件初始化、实机安装、
Web OTA 和卸载未在本轮验收。

## eSIM 页面

- 移除“完整管理”按钮和整页管理弹窗，统一使用线路级共享管理组件。
- 每个 Profile 从左至右：**详情、重命名、切换、删除**，均为小按钮。
- 使用随容器宽度变化的网格，不再最多两列；浏览器已检查 360、900、1600 px。
- 两行摘要：第一行隐藏 EID（只复制完整值，无显示按钮）与剩余存储；
  第二行 Profile 数量与下载配置按钮。
- 副标题取 eUICC 厂商信息；未知时明确显示“未知 eUICC 厂商”，不再使用线路短 ID。
- 保留详情、重命名、删除确认、下载 LPA/二维码/手动参数、确认码/IMEI 和自定义容量。
- 激活/受保护 Profile 禁删；切换后按实际回读确认。线路锁跨卸载/重开保留，
  已提交写操作的响应不明时要求刷新，防止重复写卡；A 线路结果不会写入 B 线路。

浏览器测试使用本地 fixture，所有 API 均被拦截；额外将 Vite 代理指向本机关闭端口，
不访问真实 modem。截图为测试数据：
`frontend/test-results/esim-manager-two-summary-r-b5fc1-uttons-and-adaptive-columns/esim-{360,1600}.png`。

## 数据库产物

目录：`../carrier_Bundles/data/variants/2026-10-02/`。

| 来源 | Profile | 完整 MiB | 无图标 MiB | 精简无图标 MiB |
|---|---:|---:|---:|---:|
| iPhone 16 Pro Max / iOS 27.0 | 1962 | 25.25 | 15.09 | 14.33 |
| Apple IPCC | 1846 | 21.29 | 11.10 | 10.11 |
| Pixel mustang | 1444 | 16.85 | 8.43 | 7.67 |
| Xiaomi xuanyuan | 721 | 11.64 | 2.86 | 2.60 |
| 合计 | 5973 | 75.03 | 37.48 | 34.71 |

四来源独立、每来源三变体，共 12 份。完整文件与输入逐字节一致；原始数据库内容/权限未改。
所有 Profile、匹配、来源和证据行保留。精简版总计省略 **12478 个已验证等价的可选默认值字段**，
没有整行删除运营商或 NR/VoWiFi 接入配置。

**不能保证所有标准 4G/5G/VoWiFi 注册都可由派生兜底。** 特别是当前 catalog 消费接口没有
NR/5GC 独立投影，且完整库里也存在 `ready` 但投影失败的原有行。对这些错误仍原样保留，
不能用“查询等价通过”冒充“所有注册通过”。规则、命令和逐字段裁剪报告见
`../carrier_Bundles/docs/CATALOG_VARIANTS.md`；每份产物摘要在 `catalog-variants.json`。

## 验证记录

| 检查 | 结果 |
|---|---|
| 安装器隔离行为测试 | 35 通过 |
| SimAdmin Python 回归 | 254 通过 |
| carrier_Bundles Python 回归 | 41 通过 |
| 前端单元测试 | 34 通过 |
| 隔离浏览器 eSIM 流程 | 3 通过 |
| 前端与 E2E TypeScript / 完整 ESLint / Vite 构建 | 通过 |
| Rust catalog 回归 | 25 通过，1 个实库测试默认 ignored |
| 显式实库消费者对比 | 另行执行通过，23892 次查询比较 |
| 12 份 SQLite | sealed、integrity_check、foreign_key_check、行级覆盖、SHA256 全通过 |
| 修改工作流 | YAML 与 35 段内嵌 shell 语法通过 |
| 安装脚本与打包助手 | shell 语法、Python 3.8 语法兼容、diff 检查通过 |

前端构建首次执行达到调用时限，单独重跑成功；数据库构建在慢挂载路径上超时后，改用临时
文件系统转换并缓存只读 schema，再完整重建成功。早期失败没有作为通过计入上述结果。

本机日志保存在 `.local/continuation-20261001/`，包括 `installer-tests.log`、
`project-python-tests.log`、`carrier-python-tests.log`、`frontend-*.log`、
`esim-browser-tests.log`、`catalog-all-rust-tests.log`、`catalog-real-equivalence.log` 和
`artifact-verification.json`。这些是本轮本地检查，不是新的 GitHub CI 或设备验收。

用户既有的 `docs/archive/2026-09/ESIM_IMS_PROFILE_TEST_2026-09-01.md` 到
`docs/ESIM_IMS_PROFILE_TEST_2026-09-01.md` 文档移动保持原样。
