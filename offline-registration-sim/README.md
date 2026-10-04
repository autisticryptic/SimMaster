# 派生注册离线模拟

## 执行约束

本次维护按用户要求**仅通过 GitHub Actions 编译和运行注册模拟，不在本机编译**。
`Validate Beta Refactor` 和 `Build-Release` 均执行下面两套矩阵，并上传 JSON、原始结果和日志。
工作树快照使用独立验证分支，不能拿旧 HEAD 的 Actions 结果证明当前未提交源码通过。

以下命令在 **Actions runner** 的仓库根目录执行：

```bash
python3 -B offline-registration-sim/run.py \
  --report offline-registration-sim/ci-results/standard.json
python3 -B offline-registration-sim/run.py --history \
  --report offline-registration-sim/ci-results/history.json
```

报告绑定源码摘要及 Actions repository/commit/run ID；同名结果已存在时拒绝覆盖。
远程构建包仍只生成 artifact，不自动发布 Release 或部署 410。

## 标准矩阵

默认只执行真实代码的 `offline_derivation_registration_matrix` 测试，不启动服务、不操作410。
24场景中14个完成模拟注册、10个按预期拒绝；包含LTE首包必须声明sec-agree及
不得覆盖显式disabled的场景，并新增第二安全机制成功、未提供算法拒绝、未请求的安全协商
不得覆盖disabled。LTE适配器直接调用生产Security-Client构造和Security-Server选择，不再用固定报价替身。还包含421/494累加回退、407、AKA算法、423、
UDP原报文重传、乱序消息与错误凭据/普通403等反例。内存对端独立校验Digest后才返回200。

SIM返回材料和传输保护是测试替身，不执行无线、完整IKE/IPsec、NAS、5G-AKA或VoNR。
NR仅测试命名，不声称5G注册通过。报告绑定源文件摘要且明确 `live_network_verified=false`。

## 历史条件矩阵

`--history` 单独执行 `offline_historical_registration_matrix`：18 个场景，覆盖 Globe/KPN 的
SHA1 AES/null 及已知 SHA1 拼写别名、SIM-01/02/03/04 的 UDP 路径、SIM-06 的安全报价，
以及初始/认证后 403、异常 nonce、未报价 MD5、显式 disabled 和错误 Digest 必须停止等反例。

所有身份和 AKA 材料均为合成数据。PLMN/卡名只选择历史条件驱动的模型，不是回放真实卡凭据；
历史记录未保留选中算法的卡分别测试候选算法，不能据此声称运营商接受了这些算法。
IPv4/IPv6 在该矩阵只影响 SIP 序列化，不测试真实 socket、MTU、MM/profile 换卡恢复或承载族回退。
自然续期由另外的生产 `refresh_tests` 覆盖；没有足够 SIP 参数或在 SIP 前失败的历史卡不作注册结论。

## 数据库裁剪边界

历史条件矩阵只用于回归，**不授权扩大数据库裁剪范围**。
数据库项目的可移植副本位于 `../carrier_Bundles/simulations/ims_registration/`，其中运行器
会在临时副本注入测试入口，不改变调用方checkout。对应筛选器在
`../carrier_Bundles/simulation_pruning/`。当前使用原schema v7/contract v1，直接删除符合
已测试标准模型的LTE/VoWiFi接入配置，其余保留；**不再采用v2格式/重建规则**。

最新实库验证使用已补齐小米VoWiFi的新集合，采用明确home PLMN及合成IMSI：189项删除接入
通过现有来源绑定路径落到派生，12196项其他投影与NR保持；不是189家运营商实网注册验收。
IKE源库算法组合、密钥/加密完整性、SA_INIT一致性和明确认证拒绝止损另有定向Rust测试，
不混称为本SIP矩阵已执行完整IKE网络握手。
