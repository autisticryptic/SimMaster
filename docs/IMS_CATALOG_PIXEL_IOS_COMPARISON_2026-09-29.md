# Pixel / iOS / IPCC 外置运营商库语音差异分析

> 2026-09-29；只读数据库与源码对照。LAN目标7c6cf86重装完成后才开始分析。
> 用户随后将设备/eSIM下线，本分析没有联网设备操作、换库、拨号或重新注册。
> **已确认文件/配置差异及历史代码缺陷；未固定用户原来那次测试的SIM、版本、接入腿和有效profile，不能宣布唯一实机根因。**

## 1. 结论先行

1. 这几份 catalog 不是等价完整的“基带配置转储”。Pixel 样本主要来自 CarrierSettings 与标准派生；iOS 会额外暴露部分 IMS Signaling/Carrier Bundle 字段。Pixel 的 Tensor modem 内部语义并未被当前提取器完整解码。
2. **历史客户端确实有与所述症状高度一致的缺陷**：空 Contact 参数表曾被理解成不发送 MMTEL/audio，而不是使用客户端基线。Pixel 本地样本的1444条profile全部没有显式 common Contact表，因而更容易命中。该问题已在 `2b743e29aa3c34b9aab5cc27231899c803a2fc02` 修复，当前7c6cf86包含它；不能对现在的代码简单重复“Pixel未声明语音”的结论。
3. “切换到iOS库后正常”还可能是**iOS条目不满足ready门槛而退回derived**，并非真正使用了iOS库专属参数。45400、45403、46000、46011存在实际覆盖差异，必须记录 requested/effective profile 与fallback原因。
4. 当前仍有明确的**提取器—消费者投影覆盖缺口**：`user_agent_template`、顶层`ims.security_agreement`以及部分access-specific SIP覆盖不按相同字段契约消费。它们值得单独修复/回归，但不能仅凭这些差异断定某次来电进入语音信箱的根因。
5. 若设备确实没有收到初始INVITE，应优先检查注册绑定/Contact/网络侧呼入路由及存活状态，不能先归因RTP编解码或接听按钮。网络也可能因为转移设置、注册覆盖等原因送入语音信箱。

## 2. 样本与来源

全部以 `mode=ro&immutable=1` 打开；均为sealed schema-v7、`carrier-bundles-ims-v1`，`quick_check=ok`。

| 样本 | catalog生成时间（UTC） | release/parser | profiles | SHA-256前16位 |
|---|---|---|---:|---|
| 根目录 `carrier-bundles-pixel-mustang.sqlite3` | 2026-08-08 14:55 | catalog-1be14b0e5af4f464 / android/pixel 0.1.0 | 1444 | 611449a82f45f2e9 |
| 根目录 `carrier-bundles-ios-ipcc.sqlite3` | 2026-08-08 14:54 | catalog-ipcc-4bf85b314856c162 / ios/ipcc 0.2.0 | 1866 | ffe5eedae2a5c85d |
| 根目录 `carrier-bundles-iphone16promax-26.6.sqlite3` | 2026-08-08 14:55 | catalog-23g71 / ios 0.2.0 | 1911 | de3eb7203d508419 |
| 历史目录 Pixel Mustang | 2026-08-19 13:02 | catalog-1be14b0e5af4f464 / android/pixel 0.1.0 | 1444 | 10b603c2b29c6fbb |
| 历史目录 iOS IPCC | 2026-08-19 12:59 | catalog-ipcc-5c2d27493a42812d / ios/ipcc 0.2.0 | 1866 | ed78566a02398214 |
| 历史目录 iPhone16ProMax 26.6.1 | 2026-08-19 13:01 | catalog-23g83 / ios 0.2.0 | 1911 | 70f0edbe28388604 |

历史目录为 `.local/archive/root/.codex-cf-catalogs/`；历史交接记录将这一组关联到 `v0.3.0-catalog-v7`。
文件修改时间不是固件/库生成时间，不能凭文件名把26.6和26.6.1当同一文件。

**本次逐profile比较结果：三组新旧对应库的profile_id集合完全相同，解析后的config_json差异均为0。**
这只证明这些实际下载样本的配置文档相同，不是整文件/来源元数据/图标完全相同，也不代表上游最新版本没有变化。

本地完整SHA/schema/元数据与输出：
`.local/evidence/carrier-voice-compare/{inventory,metadata,semantic-summary,mapping-coverage,target-profiles}.json`。
没有把数据库本体或用户SIM身份提交到Git。

## 3. 数据覆盖不能只看 ready

以根目录三库统计：

| 项目 | Pixel | iOS IPCC | iPhone IPSW |
|---|---:|---:|---:|
| profiles总数 | 1444 | 1866 | 1911 |
| `sip`为{} | 933 | 0 | 0 |
| 非空common REGISTER字典 | 498 | 181 | 305 |
| 非空common Contact参数表 | **0** | 114 | 222 |
| `media`为空 | **1444** | 1708 | 1645 |
| LTE ready | 1348 | 226 | 322 |
| VoWiFi ready | 984 | 101 | 141 |
| 有access-specific SIP覆盖 | 0 | 0 | 1 |

注意：iOS `sip`不为{}，有时也只是包含空register/headers/contacts数组的结构，**不是181/305以外的所有条目都具备完整信令策略**。
Pixel有498个非空register，其中456个含`user_agent_template`；不要误写“所有Pixel的sip都是空”。

相邻仓库的 `catalog_contract.py::evaluate_readiness` 主要检查域名、认证方案、APN/P-CSCF和VoWiFi ePDG/EAP/身份等路径，
不要求MMTEL Contact、接听功能实测或完整SIP对话策略。因此`ready`仅是有限静态门槛，不是语音认证证书。

根目录Pixel的`source_artifacts`为1条carrier_settings、105条icon_catalog和1条standards_reference；不是已解码Tensor基带内部策略的证明。
相邻仓库README也明确Tensor modem语义提取仍待实现。

## 4. 同PLMN也可能不是同一有效配置

以下只比较**无GID/SPN/IMSI或ICCID前缀附加条件、非exclusion**的候选；不借用同PLMN的MVNO条目。

| PLMN | Pixel | iOS IPCC | iPhone IPSW | 对照风险 |
|---|---|---|---|---|
| 45400 CSL | 无纯PLMN项；只有受限MVNO项 | LTE/WiFi unknown | LTE/WiFi unknown | 不能把MVNO当通用CSL；很可能走derived而非库参数 |
| 45403 Hutchison/3 HK | LTE/WiFi ready | LTE/WiFi unknown | LTE unknown、WiFi ready | LTE与WiFi对照结论不同，不能混为一谈 |
| 46000 CMCC | LTE ready、WiFi partial | LTE/WiFi unknown | LTE/WiFi unknown | iOS成功可能是derived；Pixel LTE缺省AKA曾有独立历史修复 |
| 46011 China Telecom | LTE/WiFi unknown | LTE/WiFi unknown | LTE/WiFi unknown | 这几份旧库不能代表当前电信派生成功是库专属成功 |
| 50212 Maxis | LTE/WiFi ready | LTE/WiFi ready | LTE/WiFi ready | 可以做同作用域静态字段比较，但仍需固定实际生效项 |

消费者 `carrier_catalog_v7.rs::load_profile` 检查对应接入的ready状态，`profile_store.rs`允许缺失/不可用来源回退derived。
所以“我选择了Pixel/iOS文件”不等于“该次REGISTER实际使用了这个文件中的profile”。

历史 [2026-09-12交接](archive/2026-09/PROJECT_HANDOFF_2026-09-12.md) 已明确：iOS/Pixel来电差异报告当时未固定SIM、程序版本、接入腿和有效profile。
相邻仓库 `SimAdmin_410_真机兼容性问题.md` 描述的是更早redfin/schema-v5、注册前就失败的案例，**不能当作今天Mustang/v7“已注册但不来电”的解释**。

## 5. Maxis 50212 的可核实字段差异

比较Pixel `profile-maxis-my-50212-2cce3fec40` 与iOS `profile-maxis-my-base-50212-12150a2817`：

| 字段 | Pixel原始配置 | iOS/IPCC原始配置 | 当前消费行为/重要性 |
|---|---|---|---|
| IMS domain/realm | 同一3GPP域名 | 相同 | 不是此样本的差异 |
| LTE APN | ims | ims | 相同 |
| LTE ip_family | ipv4v6 | ipv6 | 原始声明不同；本项目线路地址族顺序仍以线路配置为准，不能据此固定IPv6 |
| common REGISTER | 空 | `security_agreement=required`、`AlwaysAddSipInstance=true`、`PANI`、`EnableCellularNetworkInfo=true` | 不同 |
| WiFi PANI/CNI默认投影 | 缺省不启用 | 由明确字段启用 | 可影响注册身份/接入信息，值得受控验证，不是已证实根因 |
| `+sip.instance` | 原始缺省 | 明确true | **当前消费者两边均默认/设为true**，不能把原始缺失等同实际不发送 |
| 初始LTE Authorization | 原始缺省 | 原始缺省 | 当前两边均使用aka_empty基线；历史修复d7d7998已包含 |
| common Contact | 无 | 3个SRVCC相关参数 | 需结合消费者的MMTEL开关与实际发包，见下一节 |
| VoWiFi ePDG | 标准域名 | 同一域名 | endpoint相同 |
| IKE IDi | `0{imsi}@nai.epc...` | `maxis@nai.epc...` | 标准permanent NAI与运营商显式模板不同；EAP/SIM身份不能与外层IDi混为一谈 |
| IKE/ESP proposal | 缺省 | 显式AES256/SHA512/DH18等 | 消费者会过滤/补充可支持基线；不能直接拿原始数组当最终协商结果 |
| 媒体/对话/UT信息 | 少或缺省 | RTCP、ringing timer、preconditions、XCAP等更多 | 多数不会直接进入当前REGISTER；无INVITE时不是首要RTP根因 |

**iOS模式不等于天然正确、Pixel标准模板也不等于天然错误。** 不能批量抄入iOS的固定运营商IDi、终端标识或地址族来“修所有Pixel”。

## 6. 客户端解释层：历史修复与现存覆盖缺口

### 6.1 历史空Contact缺陷已修，不应再次当作现存事实

`2b743e2` 的实际代码diff修复了：
- 有profile但Contact数组为空时，同时跳过显式参数和默认参数两个分支；
- 空表推导出`include_mmtel_features=false`；
- VoWiFi release路径的基线不当受测试条件控制。

结果可能是REGISTER 200，但Contact缺少`audio`、`+g.3gpp.icsi-ref`等，注册并未正确表达MMTEL能力。
该机制与用户症状高度一致，**但提交说明里的旧现场叙述不是这次用户原测试的直接证据**。

已用Git祖先关系核实：`2b743e2`、`7e2e046`（共享接入感知Contact基线）、`d7d7998`（LTE缺省AKA）均包含于已部署7c6cf86。
当前Pixel的空Contact + voice=true 会推导MMTEL=true，交给共享Contact补全；不能再简单声称Pixel一定只注册短信。

### 6.2 当前投影覆盖缺口（本次只分析，未改代码）

1. **User-Agent字段名不一致**：提取器写`/sip/common/register/user_agent_template`；消费者读取`.../user_agent`。
   样本Pixel456项、IPCC2项、IPSW4项有template，但三库均没有user_agent字段。当前多会落入`SimAdmin IMS`，而不是提取出的模板。
   50212这组本来没有该字段，因此不能用这个缺口直接解释该组呼入问题。
2. **安全策略路径不一致**：Pixel写`/ims/security_agreement`（121项），没有common register对应键；
   消费者`project_register`只读`/sip/common/register/security_agreement`，未指定按auto处理。
   例如H3 HK 45403的Pixel原始值为顶层disabled，当前REGISTER投影仍走auto。是否需合并、优先级如何，要按契约补回归而不是静默改值。
3. **access-specific SIP覆盖未完整消费**：iOS提取器可以生成`sip.lte/nr/vowifi`，当前common REGISTER/Contact投影未合并这些层；本地IPSW实际1项有此覆盖。该事实不证明Maxis案例受它影响。
4. **非空Contact overlay的MMTEL解释风险**：当前`include_mmtel_features`只在表中有audio/icsi-ref，或表为空且声明语音时为true。
   Maxis iOS的3项SRVCC表本身没有audio/icsi-ref，故该投影推导false；Pixel空表反而推导true。
   这与共享`contact.rs`的“standard是overlay、custom才完整替代”意图需要进一步协调。
   **这是iOS样本也可能受影响的解释层风险，不支持“当前Pixel缺MMTEL而iOS一定有”的简单结论**。

源码入口：
- [catalog v7投影](../backend/src/connectivity/modems/ims/vowifi/carrier_catalog_v7.rs)：load_profile / project_config / project_register / contact_parameters。
- [profile来源与回退](../backend/src/connectivity/modems/ims/vowifi/profile_store.rs)。
- [共享Contact补全](../backend/src/connectivity/core/contact.rs)。
- [蜂窝REGISTER生成](../backend/src/connectivity/modems/ims/cellular_ims/sip.rs)。
- [标准派生配置](../backend/src/connectivity/modems/ims/vowifi/profiles.rs)。

本次未本地编译Rust，也未宣称已实际发包复现这些投影；结论来自SQLite字段和上述代码路径的静态逐项核对。

## 7. 设备上线后的最小对照要求

等用户手机测试结果和明确维护授权后再进行，不重放过去的拨号任务：

1. 固定一张卡、一个接入腿（先单LTE或单VoWiFi）、同程序SHA、同语音/资费/Trunk开关。
2. 保存catalog文件SHA、requested/effective profile、source/fallback reason；不对比一个catalog profile与另一个derived而漏记差别。
3. 记录脱敏REGISTER摘要：Contact的audio/icsi-ref/SMS/instance/reg-id、PANI/CNI、身份来源、实际安全/传输，以及200的Service-Route/Associated-URI。
4. 由用户发起一次受控呼入，判断是否收到初始INVITE：
   - 完全没收到：优先注册绑定、服务分配、活跃注册是否被覆盖、运营商转移/语音信箱设置和通道存活；
   - 收到但拒绝：检查费用/功能开关、接听端可用性、SDP/本地资源与具体SIP响应；
   - 接通无音频：再排查RTP/codec/路由，而不是反推REGISTER一定失败。
5. 单路问题明确之后，再验证用户要求的自动双注册：两开关都开、两路成功且网络允许共存，才保留双注册；否则在已启用接入中VoWiFi优先回退。

**当前已完成：本地库来源、覆盖、同作用域字段与投影代码对比。未完成：对用户那一次历史来电的唯一根因认定、现场A/B和针对这些映射缺口的代码修复。**
