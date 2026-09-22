# 参考实现 VoLTE 审计与可移植边界（2026-09-21）

本文记录对外部参考实现（中国大陆三网用户态 IMS 注册）的**只读**审计结论，用于对照当前 MM 默认后端主线。审计未解压落盘、未运行参考代码、未安装依赖、未联网、未访问设备或凭据，也未修改任何工作树。

## 1 来源与法律边界

参考归档声明为 **GPLv3**，基于上游 `3899/SimAdmin` v1.1.12 commit `306ac71` 叠加 VoLTE 增量，并引用 project-cpe、SmsForwarder、ddns-go、lpac 等来源。

因此本项目只做 **clean-room 行为与测试对照**：不复制其代码、注释或字符串。若将来形成衍生分发，需履行 GPLv3 的版权保留、修改标记、对应源代码提供及 UI 法律告知义务。

归档完整性：346 个条目，压缩 11,165,993 B，解压 19,191,948 B，最大单条 1,144,441 B；逐条 CRC 校验通过；全部路径位于单一顶层目录内，未发现绝对路径、盘符路径、`..` 穿越、符号链接、超大条目或异常压缩比。

## 2 参考实现的端到端流程

- **身份与域名**：MNC 长度优先级为 MM home operator 的 5/6 位 PLMN → EF_AD 第四字节低 nibble → MCC `460` 回退 2 位 → 其余回退 3 位；据此生成 `ims.mnc<MNC3>.mcc<MCC>.3gppnetwork.org`、`ims.epc.…gprs`、`IMPI=IMSI@home_domain`、`IMPU=sip:IMSI@home_domain`。参考**没有**分别硬编码移动/联通/电信的 IMS 参数，只有 MCC 460 兼容分支。
- **USIM/AKA**：从 QMI card status 查找 USIM AID（失败时用 `A0000000871002` 前缀），经 QMI proxy 打开 UIM logical channel 做 USIM AKA，K 不离开卡；其主流程实际只接受 AKAv1-MD5。
- **APN/profile/CID**：查找 APN=`ims` 的 3GPP profile，缺失时创建，打开 `$QCPDPIMSCFGE=<cid>,1,1,1`，保留 WDS CID，绑定 rmnet，设定 IP family 后启动网络。
- **P-CSCF/PCO**：激活前启用 reporting，激活后读 QMI settings，再用 `CGCONTRDP=<cid>` 读 PCO；若 PCO 未到，重新启用 reporting、等待约 5 秒后重读；缺失时**不**使用硬编码运营商地址。接口拉起时 IPv6 用 `nodad,noprefixroute`，并经 gateway 安装 P-CSCF `/128` host route，记录 route/address journal 以便回收。
- **REGISTER/IPsec**：明文 UDP/5060 初始 REGISTER，`Expires: 3600`、`sec-agree`、完整 `Security-Client`、`P-Access-Network-Info`、Contact 带 `+g.3gpp.smsip`；401 后解析 AKAv1-MD5 challenge（Base64 nonce ≥32 字节，前 16 RAND、后 16 AUTN），装 `hmac-md5-96` + null 加密的 ESP transport XFRM，再以 Security-Verify 发第二个 REGISTER，只接受最终 200 且要求 P-Associated-URI。
- **状态码与刷新**：主要处理 401，没有成熟的 407 处理，403/423/400/超时多直接失败；失败后以 2–120 秒指数退避**重建整个 bearer/session**；没有真正的 authenticated refresh REGISTER。

## 3 与当前主线的对比结论

当前 SimAdmin 在注册状态机上**明显更完整**：支持 401/407、423/Min-Expires、有界 UDP 重传、Call-ID+CSeq 关联；每次刷新重算 Digest/nonce-count/nextnonce；lease 未过期时保留 protected bearer，成功后才提交新 XFRM；403 视为终止态，仅特定 421/494 且 `auth_rounds=0` 才做一次兼容尝试。

参考实现对本项目最有价值的部分是 **P-CSCF/PCO 时序**（激活前开 reporting、PCO 未到时有界延迟重读），这与 SIM-04 在 T03/T04 的实机结论一致：真正的变量是 attach/reporting 时序。

一处**确实存在的差异**值得记录：参考在 MCC `460` 下有 2 位 MNC 回退，而当前 `cellular_ims/identity.rs` 只接受与 IMSI 前缀匹配的 MM home operator 或 EF_AD，两者都缺失时返回 `home_plmn_mnc_length_ambiguous`。这属于可按需评估的兼容项，不影响已验证路径。

## 4 可安全移植的通用机制

MNC 长度来源及 provenance；MCC/MNC→三位 MNC 的域名/APN 格式；单个 REGISTER 生命周期内冻结 IMPI/IMPU；P-Associated-URI 仅用于后续 originating identity；单一 profile/CID 激活所有权；`QCPDPIMSCFGE` 仅负责 PCO reporting；`CGCONTRDP` IPv6 点分 octet 解析；PCO 优先于 DNS 且 DNS server 不得当作 P-CSCF；本地/P-CSCF 同族、gateway、host route 校验；USIM 内 AKA 并严格校验 RES/CK/IK/AUTS；按 Security-Server 原样生成 Security-Verify；SPI/port 范围校验与安全回滚；scoped XFRM 清理；Call-ID+CSeq 关联与 RFC3261 UDP 重传；bearer/reporting/XFRM/socket/worker generation 的明确 owner；有界 REGISTER 兼容变体预算。

## 5 明确不移植

固定 `mmcli -m any`、固定 `/dev/wwan0qmi0`/UIM slot/网卡名、固定端口与 REQID；自动拉起 qmi-proxy；无设备授权地执行 profile 创建、`CGDCONT` 重写、`CGACT=1`、`$QCPDPIMSCFGE`；`ip xfrm state/policy flush`；固定 IPv6-only 或 AKAv1-only；只处理 401 而不处理 407/423；失败即拆 bearer 重建；把 IK/CK/RAND/AUTN 或完整 SIP challenge 写入日志或状态。

## 6 建议的离线测试方向

P-CSCF 解析（IPv4 4/8 组、IPv6 16/32 组、延迟读取、同族过滤、DNS 不冒充 P-CSCF、CID/APN 不匹配）；identity（多 PLMN、EF_AD 2/3、漫游 VPLMN 不改 home domain）；bearer 契约（短/完整 APN、多 CID、IPv4/IPv6/双栈、缺 gateway、partial-family 不误开第二 bearer）；SIP transcript（无响应、401、407、AUTS、缺 Security-Server、403、423、Contact expiry、CSeq/Call-ID 关联）；XFRM 只测 argv、脱敏与 scoped uninstall，不执行 `ip`。

实机采集一律脱敏，只取 stage/error/APN/CID/ip_type/family/PCO count/P-CSCF source/route applied，不采集 IMSI、nonce、IK、CK、AUTN 或完整 SIP。
