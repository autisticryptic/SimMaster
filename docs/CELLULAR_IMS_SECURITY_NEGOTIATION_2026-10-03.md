# 蜂窝IMS实际协商补强与410验收

## 最终设备状态

410访问 **http://192.168.68.1:3000**。已部署本地验证快照
`98d0e093170ddc18767de2507833be2b741d1100`，版本1.1.5，PID348115。
这轮改动直接作用于蜂窝IMS注册与认证/续期路径，不是仅VoWiFi或数据库裁剪变化。

- 正式服务使用 **derived / IPsec** 注册，last_error=null，reconnect_count=1。
- 同一次registered_at连续采样超过180秒，额外只读收尾仍正常。
- **本次实际选中了第二个SHA1/null机制**：命名空间内两条SA的完整性为hmac(sha1)，
  内核空加密算法名为ecb(cipher_null)。不是仅验证原AES路径。
- null表示IPsec完整性保护但不提供额外信令加密，不是没有IPsec/AKA。AES仍在客户端报价首项，
  不代表网络一定选择AES；显式严格AES-only配置仍不能接受null。
- MM仍PID957、boot保持，未重启MM/基带或清恢复预算；本boot未检出所筛选的fatal/crash记录。
- 配置、数据库、systemd unit及既有族策略保持；前端仅同步新的安全提案错误提示，20个资源HTTP/磁盘校验通过。
- 守卫accepted退出，recovery timer恢复active，无新遗留维护hold。新会话续期计数仍为0，
  不能把此前版本的自然续期计数合并到本次。
- 运行库仍保留在设备；验收证明实际选择了派生分支，**没有卸载全部数据库来做物理无库测试**。

## 修复的具体运行时缺口

### 多机制报价不再只发首项

旧代码允许列表有多个机制，却在Security-Client构造时只取`.first()`；服务器选择逻辑却能考虑整个列表。
完整Pixel库中有14个LTE-ready条目明确列出null和AES，另2个只列SHA1/null，说明这不是假想差异。

现在完整发送有界、已验证可安装的允许列表，保持顺序。每个替代机制携带**同一组**预留SPI/端口，
不为每个机制新建承载或重做AKA，不添加客户端q参数、不静默截断、不容忍无效token/未知算法。
认证请求重复原始完整Security-Client；Security-Verify重复完整服务端列表；仅安装被选中的SA。

派生LTE明确提供SHA1/AES-CBC及SHA1/null，并启用严格机制匹配，拒绝未声明MD5等算法。
这收紧了旧派生的宽松接受范围，而不是任意接受服务器要求。显式catalog机制顺序/限制保留，
VoWiFi派生策略没有在本轮被顺便修改。显式disabled遇到未请求的协商也会在AKA前拒绝。

### 空加密算法不再错误携带CK

旧AKA→XFRM路径把CK无条件复制到SA，连cipher_null也收到非空加密密钥。
Linux空加密算法要求零长度密钥，原参数会返回EINVAL。

现在仅对cipher_null清空安装计划的加密密钥，保留IK完整性密钥；AES仍保留CK。
AKA材料本身没有被清空或篡改，Digest计算仍使用原始正确材料。

真实本地Linux私有网络命名空间对照：

| 输入 | 内核结果 |
|---|---|
| 旧null算法＋非空CK | 拒绝，Invalid argument |
| 新null算法＋空加密密钥 | 接受 |
| AES算法＋CK | 接受 |

这是内核参数安装验证，不是本地模拟真实运营商；随后410实际选中null并注册成功提供了额外实机证据。
采证只保存算法名称，不保存设备XFRM密钥。

### 冻结端口与严格选择一致

已预留端口必须与本次请求的冻结端口一致，不再直接返回不匹配的旧预留值。
续期重协商保持port_us，变更port_uc/SPI，并在失败时回滚临时套接字/SA，不动原有效关联。
严格服务器选择允许协议默认的ESP/transport省略写法，但不把算法变化或隧道模式当作默认值。

新增错误码`cellular_ims_security_client_invalid`及对应中文提示，不把本地配置错误冒充SIM或网络拒绝。

## 验证

- 369蜂窝Rust测试＋217关联Rust测试，共586项通过。
- 24个真实REGISTER代码矩阵场景：14正例、10预期拒绝。LTE适配器直接使用生产报价和选择函数，
  不再用固定Security-Client替身；新增第二机制成功、未声明算法拒绝、disabled保护。
- 认证/续期路径的环回UDP测试覆盖同一原始报价、正确选择/安装、真实请求收发及回滚后原SA保留。
  SIM材料、worker XFRM执行在此单测中是测试替身，不冒称运营商网络。
- 262项目Python、98数据库Python、36前端单测、8隔离浏览器用例、类型/lint/build通过。
- 初轮发现错误码计数常量需同步、旧静态守卫仍引用“只发首项”测试名，均修正并全量重跑。
- 本机host namespace的大UDP连纯Python环回也丢失，导致新1481字节测试包超时；私有NET/PID/DBus
  对照及全量回归通过，未缩短生产包或弱化测试断言。首次ARM64构建超时，后继完整构建通过。

包SHA256：`8d975e3fa4e3f7d583b80c25ccecae6e38d28b4d851c8c694746fad84a57815e`。
程序SHA256：`f77c2d4753b69437f502ebc78ce7dcce27db8ab027f343d7cae680a47ad56de1`。
源码摘要：`a8b2d87896efc19a818d0cc8334bae430af52cc3a66b895f45e940a20fc731c8`。

SimAdmin工作HEAD和用户索引未变，构建快照位于`refs/build-snapshots/cellular-security-20261003-a8b2d87896ef`；
没有推送SimAdmin或发布Release。数据库仓库同步的24场景脚本/证据与说明已推送 **2966036**，
**本轮没有扩大裁剪规则或换设备数据库**。

证据目录：`.local/evidence/cellular-security-20261003/`，重点为`kernel-xfrm.json`、
`cellular-private-tests.log`、`related-rust-tests.log`、两套矩阵报告、`package-verified.json`、
`deploy-stage.json`、`deploy-install-accept.json`、`closeout.json`。
设备备份：`/opt/simadmin-staging/cellular-security-20261003-a8b2d87896ef/backup/`，保留供维护。

## 未覆盖边界

不能据此保证所有运营商或SIM均注册。私有端点、专属身份、订阅开通、特殊算法及策略仍可能依赖数据库。
未增加认证失败后的盲试、无限REGISTER或基带复位；默认双栈→IPv6→IPv4保持。
本轮没有新的真实VoWiFi、通话/音频、自然续期或故障注入验收，不混用此前结果。
