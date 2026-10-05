# 中国移动 421→403 定向修复候选（2026-10-05）

## 当前交付状态

最终候选 **`6ddc75157a106513aa90f80b7b4b0b15d66251a1` / 1.1.5** 已通过GitHub Actions并校验双架构包。
这是根据用户提供日志进行的**有界兼容性修复尝试**，尚未部署到设备，也没有证明中国移动实网注册成功。
本轮没有本机编译、操作SIM/MM/基带、换库、重复注册或改用户配置；master/用户HEAD与索引保持。

该候选最初在 `dev/ims-cmcc-20261005T053527Z-d295ae33` 验证，后续已整合至master（ae3926e），
临时分支已清理；下文固定Actions运行仍可用于取回原候选制品。未发布或覆盖正式Release。

## 日志所证明的问题

输入为 `simadmin-diagnostics-2026-10-05.log`（81行）、`message.txt`（137行）及用户先前启动日志。
其中实际配置明确为 `derived_3gpp_lte_46002 / profile_origin=derived`，缺少database配置后的派生回退已生效。
申请双栈、授予IPv6，CID6的P-CSCF归属验证通过，取得两个P-CSCF。

第二P-CSCF连续出现：

1. 标准初始REGISTER含空AKA身份、Security-Client和Require/Proxy-Require sec-agree。
2. 响应421，Security-Server数量1、Warning存在、没有Require和Digest挑战，auth_rounds=0。
3. 旧逻辑进入generic候选，去掉Authorization和安全声明。
4. generic收到403 / Authentication Failure，仍auth_rounds=0。

第一P-CSCF则两种候选各约32秒无完整响应。失败后的cleanup pending不是前面421/403的直接根因。
日志只提供Security-Server数量，**没有实际机制/算法及421 Warning正文**；一个头也可能含多个报价。
不能据此认定SIM AKA计算错误、未订阅VoLTE或强制修改归属PLMN/realm。
旧版本5c378f8与部署版71c970d在有关候选切换函数上相同；这不是后继资源恢复代码新引入的问题。

## 最小、有限的行为变化

新 `security_hint::decide` 在原动态/static generic回退之前做三态决策：NotApplicable / Retry / Stop。

只有以下条件齐全时，允许一次额外候选：

- 标准派生、初始未认证流程（auth_rounds=0）；状态421或494，存在Security-Server。
- 原请求已经含完整安全声明、空AKA身份和多机制Full报价。
- Security-Server经过原有有界严格选择器验证；机制必须来自实际原报价，不能猜测/补齐缺失参数。
- 响应Require/Proxy-Require若存在，只允许sec-agree；不把421当成401/407，也不从Warning文字推断算法。
- 仅可收窄为**原首选AES机制**，不允许未认证421提示把新重试收窄到null/MD5/其他未知策略。

新候选名：`standard_3gpp_security_hint_reoffer`。它保留身份、realm、registrar、空AKA、PANI、业务特性和
Require/Proxy-Require，仅收窄实际报价集合。采用新的本地预留SPI/端口，不复制421的服务器tuple。
这次重报价的Security-Client随后在401/407、AUTS、423中保持冻结；Security-Verify取自真正的认证挑战，
不是旧421提示。401/407选择必须匹配**实际发送的单一机制**，不能退回共享profile的其他机制。

一旦进入此分支：重复421、403、异常报价或失败均停止该P-CSCF的候选尝试，不再回到generic/其他头探测。
不接受无保护初始200；只有现有AKA/IPsec成功路径才能完成注册。成功后的session/refresh/unregister保留
该机制限制，续期失败仍按原保护逻辑保留有效旧SA，不以重报价改变其他会话。

### 保持不变

- 候选预算24、AKA轮数2、UDP重传定时器及P-CSCF顺序不变。
- 普通403仍终止；原无此提示的generic/Require驱动兼容路径保留。
- 原full报价直接获得合法SHA1/null挑战的路径保留；只是**新增未认证提示重试**不能主动降低保密性。
- 双栈→IPv6→IPv4、配置来源回退、SIM身份域与PLMN推导、VoWiFi策略不变。
- 不根据46002/46000不同便强制发送visited-network或改归属域。

## 诊断及审查补强

增加独立的隐私安全诊断，只输出固定类别/布尔/有界数字：

- `security_mechanisms`：ipsec-3gpp、tls、unknown。
- `security_integrity`：已知SHA1/MD5标签或unknown。
- `security_encryption`：aes-cbc、null、unknown。
- `warning_codes`、`warning_classes`、`diagnostics_malformed`。
- `response_cseq`仅合法数字，不原样输出远端CSeq尾随内容。

不输出nonce、身份、域、原始Warning/agent、未知扩展值或SPI/端口；诊断不参与接受/重试决策。
诊断头段上限16KiB、条目上限16，歧义/越界只标malformed。

审查发现并修正了两项边界：
1. Security-Server未知参数不能以“引号总数平衡”绕过语法检查，如 `x=bad"value"` 或相邻引号字符串必须拒绝。
2. REGISTER事务关联拒绝CSeq多余字段；日志也不回显它们，避免把不可信内容当作已脱敏字段。

## 验证证据

最终源码对应的运行均为success：

- [Build-Release 37268498822](https://github.com/autisticryptic/SimMaster/actions/runs/37268498822)
- [Validate Beta Refactor 37268498818](https://github.com/autisticryptic/SimMaster/actions/runs/37268498818)
- [Frontend Checks 37268498810](https://github.com/autisticryptic/SimMaster/actions/runs/37268498810)

两套下载的测试artifact逐名确认 **68项相关回归**：43既有、9 hint、12诊断、3续期/注销、1 CSeq。
每套还执行：

| 矩阵 | 场景 | 模拟成功 | 预期停止 |
|---|---:|---:|---:|
| 原标准注册 | 24 | 14 | 10 |
| 原历史条件 | 18 | 12 | 6 |
| 新安全提示 | 12 | 4 | 8 |

新矩阵包含421/494、SHA1别名、407代理挑战，以及null/MD5提示、重复421、403、挑战逃逸、缺少安全报价、
未保护200必须停止。正例使用**合成完整AES报价**，不是补造或重放真实CMCC的未知421内容。
真实refresh/423、AUTS和受限refresh/unregister拒绝测试独立执行；未运行实网故障注入。

官方ZIP digest、源码逐文件摘要、包内30文件SHA清单、ELF及meta完整核验：

| 架构 | artifact ID | tar.gz SHA256 |
|---|---:|---|
| ARM64 | 11328140069 | `08962fe848c97154098c8100129b0fd7751c1058ab83dee8e3f3387a2f302895` |
| AMD64 | 11327402070 | `632ae43deec848ee03622f0776498e367cd26f2627875c54f1136d170c2d97cd` |

本机证据：`.local/evidence/cmcc-20261005/actions/{current,verified,final-proof}.json`及对应ZIP/日志。
首候选29cf7a5也通过，但审查后增加上述语法/隐私和生命周期覆盖，以最终6ddc751为准。
本机仅执行Python静态守卫（286项通过），Rust/前端/注册矩阵全部在Actions执行。

## 下一次实网测试

请使用上述Build运行中的 **pkg-arm64** 候选，而不是旧Release的5c378f8；启动/关于页应确认commit为6ddc751。
同一设备不要同时运行临时目录程序与正式服务，也不要同时换库、改SIM身份域或重启MM/基带，避免混淆变量。

重点保留：实际profile来源、首个421的新增诊断、是否出现 `standard_3gpp_security_hint_reoffer`、
是否收到401/407、是否完成受保护注册，以及停止分支的固定reason。
若提示不可用/较弱报价，则本候选会安全停止并留下更具体诊断，而不是强行猜算法继续。

**尚不能宣称中国移动问题已实网修复。**本轮没有访问或修改410；最近71c970d设备验收是前次Globe记录，
不是当前CMCC候选或用户这份日志对应设备的实时状态。
