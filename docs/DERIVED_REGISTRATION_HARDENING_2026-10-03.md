# 派生IMS/VoWiFi补强与410部署（2026-10-03）

## 已部署并验证的状态

410访问 **http://192.168.68.1:3000**。正式后端为本地已验证快照
`cc918f5b28a4efc24573e2ec8895d06c89530663`，版本1.1.5，PID255232。
该快照保存在 `refs/build-snapshots/derived-20261003-4f0cc99ddc90`，没有移动用户工作HEAD或改变用户索引，
没有发布SimAdmin Release或宣称通过新的线上CI。

- 正式蜂窝IMS实际使用 **derived / IPsec** 注册，last_error=null，reconnect_count=1。
- 同一registered_at连续采样超过180秒；额外只读收尾仍为同一会话、同一PID。
- 申请族/自有profile均为4（IPv4v6），实际授予IPv6。默认双栈→IPv6→IPv4策略不变。
- MM仍PID957，boot未变、QMI存在；当前boot未检出所筛选的内核fatal/crash记录。
- 原配置表、配置文件、数据库、前端及systemd unit保持；本轮只替换后端及其metadata。
- 守卫已accepted退出，recovery timer恢复active，无本轮遗留drop-in。未重启MM/基带或清恢复预算。
- **新版本自然续期计数仍为0**，不能把部署前旧版本的6次续期算到本次。
- VoWiFi开关仍保留用户原值（关闭），**本轮没有真实VoWiFi注册验收**；协议回归不能冒充实网成功。

包SHA256：`74b4d58dda481265a61ec70342209ac6db88a64450af37831b9542c0863996a8`。
程序SHA256：`717d4de0f61d3ae11616fc1eaea42c1e471b51265e6969631bf6cb0a63967991`。
源码摘要：`4f0cc99ddc9076facb8098f2ea2c5cb71ecec6275738f683a8dfaa1a128bf920`。

## 根据完整数据库实施的补强

### 1. 补齐同一DH组内的已支持提案

标准派生新增：

- `aes256-sha512-prfsha256-modp2048`
- `aes128-sha512-prfsha512-modp2048`
- CHILD提案 `aes128-sha512`

前两种分别见于完整IPCC/iPhone库的13项和4项IKE提案记录；这不是13/4家运营商实网成功的统计。
它们均使用已有实现和同一个MODP2048组，不新增弱算法、不增加新的DH组尝试。
原首选保留，原2个DH组×2条传输路径预算保持。只有派生配置改变，显式catalog配置不被覆盖。

测试使用实际派生提案生成SA_INIT、让合成对端只选择新增组合，再执行密钥派生、IKE_AUTH实际加解密，
篡改认证数据必须被拒绝；SA_INIT仍低于测试限定的1400字节。
这不是声称完整ePDG/EAP/运营商网络流程已完成。

### 2. 在SIM认证前拒绝不一致的IKE初始响应

SA_INIT新增事务ID、发起方标志、重复必需负载、KE组/长度/保留位和nonce长度校验。
不一致时不记录新的对端密钥材料、不进入EAP/SIM阶段，减少把协议问题误当作SIM认证故障的机会。
此检查不授权换身份、降级安全或增加承载重试。

### 3. 对明确IKE_AUTH拒绝停止重复认证

已解密确认的authentication_failed/authorization_failed不是UDP端口、地址或DH组问题。
这两类错误现在在传输路径/提案、地址、域名和地址族各层向外返回，不在一次连接中继续重复SIM认证。
超时、提案不匹配及明确地址族提示仍走原有有界逻辑；没有把所有错误笼统设成不可恢复。
上层原有调度/冷却机制仍在，不把一次拒绝永久锁死所有SIM。

### 4. LTE required判定与真实首包行为对齐

SIP模拟从19扩到21场景：13正例、8预期拒绝。新增严格首包sec-agree要求的LTE正例，和不能覆盖
显式disabled的反例。LTE派生首包原本就具备相应声明，本次是用行为证据修正数据库的过粗判定，
**不是声称本轮新发明了LTE安全协商**。

数据库规则仅对LTE允许已验证的required形态；WiFi仍是挑战驱动，不能由LTE结果推导所有WiFi
required首包策略都可删除。私有端点、专属身份、证书、开通及未覆盖媒体/业务策略继续保留。

## 数据库与验证结果

独立数据库仓库提交 **74bfd6cdfc2d33e91d92b90dcb61e36b347e2849** 已推送并核验远端main。
新的模拟证据按当前源码重新生成，不用旧19场景报告授权新规则。

新12库：`../carrier_Bundles/data/variants/2026-10-03-derived-hardening/`。
所有完整性、外键、SHA256与实库消费者回归通过：189项删除接入走现有派生，12196项其他投影及NR保持。
**实际实库删除数量没有增加**：这些记录仍受其他未证明的策略约束，不能为提高删除比例忽略它们。
新产物仅本地生成，410继续使用原已验证Pixel库，没有在升级代码时顺便换库。

- 485项定向Rust回归通过（含私有PID/DBus生命周期、IKE、SIP、派生及registry）。
- 261项目Python、98数据库Python测试通过。
- 21个SIP模拟场景通过，13次模拟注册、8个预期拒绝。
- 首次扩大live测试发现旧折行测试fixture缺SPI/端口却期望形成完整Security-Server报价；
  修正fixture补齐合法字段后86项live测试全部通过，没有放宽生产报价验证。
- 实库测试首次本地编译超时，随后完整重跑通过，失败日志保留。

## 仍然保留的边界

这是一轮有来源依据的增量补强，不是证明已经穷尽所有无库注册差异。
新增更强DH组、特殊证书/身份、私有域名、开通限制和全部真实SIM/VoWiFi场景仍需独立验证。
注册成功不替代通话/音频、实际VoWiFi隧道或跨运营商验收。

本轮证据：`.local/evidence/derived-hardening-20261003/`，包括源码/包证明、两套模拟报告、
数据库推送与实库回归、`deploy-stage.json`、`deploy-install-accept.json`、`closeout.json`。
设备备份：`/opt/simadmin-staging/derived-20261003-4f0cc99ddc90/backup/`，保留供维护，不作为孤儿删除。
