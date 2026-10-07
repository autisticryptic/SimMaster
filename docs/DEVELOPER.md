# 开发、验证与交付

本页是代码工作流入口；架构见[ARCHITECTURE](ARCHITECTURE.md)，安装见[INSTALL](INSTALL.md)，
当前未完成事项只维护在[HANDOFF](HANDOFF.md)和[DEVELOPMENT_PLAN](DEVELOPMENT_PLAN.md)。

## 1. 分支与编译约束

- 日常仅维护`master`，GitHub远端为`autisticryptic/SimMaster`（本地remote通常为`simmaster`）。
- 不为每次验证长期留下分支。确需临时分支时，合并并确认提交已保留、CI通过后删除。
- 不force-push主分支，不清理未知用户工作，不提交私有SIM材料、日志或凭据。
- **Rust/前端构建、编译后的测试及注册模拟只在GitHub Actions运行，不在本机编译。**
- 本机可以读/编辑代码、运行不构建的Python/静态检查、查看Git差异、下载并验证产物。
- 不把本机旧二进制、旧HEAD的测试报告冒充当前工作树通过；报告绑定完整commit和源码摘要。
- 运营商无关的兜底修复用协议/状态机测试，不引入MCC/MNC专属代码或专属测试掩盖全局问题。

## 2. 项目布局

| 路径 | 职责 |
|---|---|
| `backend/src/api/` | HTTP、认证、请求与响应模型 |
| `backend/src/connectivity/core/` | 接入无关SIP/AKA、注册及业务模型 |
| `backend/src/connectivity/modems/ims/` | 蜂窝IMS、VoWiFi、profile适配 |
| `backend/src/hardware/` | 协议后端、SIM与设备驱动 |
| `backend/src/services/` | 线路/worker、业务路由、Trunk、自动化、通知 |
| `backend/src/platform/` | 配置、SQLite、DNS、命名空间和系统工具 |
| `frontend/src/api/` | API契约和封装 |
| `frontend/src/pages/`、`components/`、`hooks/` | React工作台与组件 |
| `bruno-api/` | API调试集合 |
| `.github/workflows/` | 构建、回归及发布门禁 |
| `scripts/`、`deploy/` | 统一打包、安装器和驱动资源 |
| `offline-registration-sim/` | 无设备REGISTER协议模拟，不能当实网证据 |
| `.local/` | 私有诊断/备份/历史材料，不提交 |

## 3. 修改边界

新接口/能力应同时核对：后端DTO和认证、路由、前端类型/封装、实际页面、Bruno请求、兼容编码和测试。
所有线路操作显式解析line_id；禁止重引入“第一台modem”隐式全局控制。
设备能力按driver声明，不从名称猜QMI/AT能力，不在通用层硬编码某个QCM410端点。
同一物理端口只有一个owner；串行锁不代替SIM、MM owner、代次及资源归属验证。

展示投影与可操作runtime分离：恢复门禁可以阻止写入和承载，但不能把原线路页面替换或清空。
保留原工作台的列表/详情/标签/保存意图，仅在必要位置提示未知/恢复中，并禁止对应危险操作。

## 4. 配置与API

配置文本和SQLite分层由`platform/config_file`、`config_store`及`ConfigManager`负责。
YAML写后重解析，保留未变注释；线路映射/业务历史不因重构批量改写。
新增字段必须有默认/迁移规则，拒绝重复别名及不支持的顶层键。

除明确公共的健康和认证入口外，业务API默认受会话保护。不要为了调试绕过认证、重置账号或回显密码。
每个请求确认方法是否实际有副作用：GET命名不自动保证不会启动探测或清理旧对象。
契约细节见[IMS协议](IMS_REGISTRATION_POLICY.md)和[原生后端](NATIVE_BACKEND_STATUS.md)。

## 5. Actions验证

主要工作流：

- `Validate Beta Refactor`：硬件无关Rust、配置/兼容边界、私有D-Bus/HTTP回归及前端检查。
- `Build-Release`：前端、后端测试、ARM64/AMD64构建、同版本完整包和发布门禁。
- `Frontend Checks`：路径匹配时运行lint/类型/构建与隔离Playwright工作台回归。

新Rust测试必须加入执行过滤器；仅`cargo test --no-run`通过不表示执行过测试。
过滤器选中0条应失败；下载日志逐名核验新增回归，避免只看绿色workflow。
私有D-Bus和fake peer必须与真实系统总线、modem和公网隔离。

注册模拟在Actions runner执行，例如：

```bash
python3 -B offline-registration-sim/run.py --report offline-registration-sim/ci-results/standard.json
python3 -B offline-registration-sim/run.py --history --report offline-registration-sim/ci-results/history.json
python3 -B offline-registration-sim/run.py --security-hint --report offline-registration-sim/ci-results/security-hint.json
python3 -B offline-registration-sim/run.py --fallback --report offline-registration-sim/ci-results/fallback.json
```

四个矩阵分别为 24 标准、18 既有历史、12 明确安全提示、32 全局候选继承场景；精确数量和 suite ID 由运行器校验。
新增两个协议矩阵统一使用合成 001/01，不按运营商决定行为。状态继承、去重和真实 AKA 前门禁另有实际 Rust 单元回归。
历史名称只是已有fixture标签，不得将某卡特例模型当作全局兜底正确性的证明。
增加验证应围绕请求继承、状态转换、授权/算法边界、次数/时间预算和错误分类。
预期拒绝是通过的负例，不是网络注册成功；模拟也不能代替无线/SIM/运营商订阅/真实安全通道。

前端依赖使用锁文件安装；隔离浏览器fixture必须拦截API并禁止连接真实设备。
真实页面调整需要验证原布局、选择保持、保存开关、blocked→ready切换及没有额外硬件请求，
不能用另一个临时页面代替用户界面验收。

## 6. 产物与发布

`VERSION`及三个依赖清单由`sync_release_version.py`同步。`scripts/pack-ota.sh`调用统一打包器，
程序、www、meta、同版本安装器/unit/设备资源和SHA清单来自同一commit。
普通push只生成artifact，`Publish Release`应skipped；正式发布需master上显式授权、新tag且完整门禁通过。
版本号不代表代码先后，必须核对commit、ELF、程序/前端和包SHA。

验证下载时：官方artifact digest → ZIP/包外SHA → 安全解包布局 → 完整文件SHA清单 → meta/ELF架构/commit。
校验来自可信发布方，不能把SHA当作数字签名；不执行未知下载内容以验证可信性。

合并docs-only收尾可以不重编译，但须证明构建输入与已验证代码提交相同，不伪装成新二进制。
旧Release/tag不能为清理分支被移动，旧制品不自动获得新安装器安全保证。

## 7. 设备验收

按[INSTALL](INSTALL.md)安排受控窗口，先看实时SIM/MM/注册/通话/管理路径，而不是重放旧脚本。
保留配置、运行库、归属账本和未知结果；备份与回滚范围明确。停止自己的服务不等于可以重启基带或清预算。

最小交付应区分：

1. 编译通过；
2. 自动化测试实际通过；
3. 产物摘要/版本核验；
4. 部署程序/HTTP/配置保持；
5. 当前SIM的初始注册；
6. 同会话自然续期；
7. 通话、短信、音频及故障注入。

上一步不能替代下一步。当前卡成功不能替代另一运营商；一次重新注册不能冒充自然续期。
本地历史凭据、完整日志、号码和数据库不得放入公开报告。

## 8. 文档规则

[docs/README](README.md)列出固定主题入口。新事实更新相应手册和简短HANDOFF；
历史验证只写[历史摘要](archive/README.md)，详细原始材料留Git历史及`.local`。
不继续新增日期命名的多份“当前交接”、重复Prompt模板或逐工具调用流水账。
