# 当前接手与项目状态

> 更新：2026-09-30。**本文件是唯一当前接手入口**；历史记录在 [archive](archive/README.md)，
> 私有操作材料在本机 `.local/`。不要根据旧文档的“当前版本/下一步”重放操作。

## 当前已部署：正式主服务 IMS IPsec 注册成功（2026-09-30 19:14 UTC核验）

**已按用户新授权完成生产注册修复与部署：正式程序为 `1.1.5 / 48e269ca8b536ef7ba82ed98b58d3623540f0547`，PID119010，主服务运行中，不是维护探针。** 本节优先于下方旧安装cf13a66、主服务停止或“候选未部署”的历史记录。

- **19:07:23 UTC正式注册成功**，实际`derived_3gpp_lte_51502 / ipsec / wwan0 / ipv4`。19:11–19:14五次独立只读采样均保持registered、同一个registered_at、last_error=null；收发时间继续推进、活动通话0。首次连接计数reconnect_count=1未增加，服务NRestarts=0。与最初注册时间相隔超过7分钟，不是单次瞬时快照。
- **生产profile生命周期已实际启用**：v2账本、`runtime.phase=active / abandoned=false / process_id=119010`，自有动态CID4为IPv4/ims，关联原MM owner和Bearer/155、Modem/77、实际wwan0/本线路namespace。**活跃profile与bearer receipt是正常在用资源，不要删除或按孤儿记录处理。**
- 原地址族顺序未变：本轮双栈准备被校验拒绝（`mm_ims_profile_lease_unverified`，未断言其唯一根因），IPv6连接收到GGSN拒绝，随后按既有流程新建exact-family IPv4 profile并成功。没有手工固定IPv4、覆盖CID1/2/3，或增加SIP超时后的承载循环。
- **部署已完整核验**：运行SHA256 `38558365459bc01285b297ebcdf7c899e0d0a16ff9481a0b1d15f0dad91f9c90`、meta48e269c和前端MD5 `01505b0195870511cc8428e1d730b53c`匹配ARM64制品。复制窗口config.yaml/data.db哈希相同，没有重建DB/建备份/清预算；服务启动后的正常运行写库不等同于覆盖原DB。MM仍PID1028，未重启MM/基带；管理仍usb0，recovery timer已恢复active。
- **遗留secondary服务入口已修正**：停掉旧`secondary-qmi-init`无效重启循环，安装包内canonical `device-init` unit；目前保持inactive，未在线执行硬件初始化。主线路普通数据仍关闭，其配置未变；没有借此重启MM或改变USB。
- 最终Validate **36760292016** / Build **36760292160**全success，实际下载日志核验 **83累计新增回归+11兼容/更新回归**均ok，双架构制品digest/meta/ELF/程序/前端校验通过。Linux235 Python与定向格式/diff通过。16bf1f9虽然workflow为绿色，逐名核验发现Validate漏跑新增423注销回归；48e269c补齐门禁后重新验证，未拿旧包替代。
  ARM64 artifact11118397115、包SHA256 `5785843b997b1a8c2ba37b1bd51b02df03c433913f4c342e62f19fb63cdab361`；AMD64 artifact11119515020、包SHA256 `7fc4a2b7852f81dd99a474f6a692b57f2bcb44694e6a53c7397b3c4a38535f61`。Publish skipped，旧Release/tag未动。
- 证据 `.local/evidence/ims-route-completion/48e269c/{verified.json,production-verified.json,production-install.json,production-active-profile.json,production-stability.json,production-final-facts.json}`。初次stage把正常子UE worker误判为额外程序，另一次遇到MM对象换代空窗；均在只读预检停止、未上传/停服，失败记录已保留。后续按真实父子进程关系和有限只读稳定库存核实后才操作。

### 明确保留的未完成项

1. **运营商注销仍返回SIP500/rejected**：修补了已认证Digest/nonce-count与Security-Verify，以及禁止423将Expires0变正数；48e269c维护窗口内再次成功注册，但注销仍被500拒绝，本地承载/namespace/profile回收均通过。不能声称网络注销修好；该项独立待排查，不再为此打断当前健康注册。
2. **当前版本自然续期计数仍0**；未做长通话、呼入/音频、真实换卡/故障注入验收。不能把历史其他卡的续期结果或此次短窗稳定观察代替这些项目。
3. 普通数据共存不在首版新profile准入范围（当前数据关闭）；MM owner/SIM/boot变化、未知持久化/写入仍保留记录并阻断，需要维护，不承诺跨基带/MM重启自动删除旧profile。
4. 若再次部署或需要注销诊断，必须重新确认当前注册/通话状态和维护窗口，不根据下方“主服务停止”的旧快照直接停服。用户原有历史文档移动仍未提交且未改动。

## 生产候选开发与部署前现场（2026-09-30，以下为历史步骤）

用户在探针成功后明确要求完成项目修复并部署。**新现场不再是下方“服务停止”**：本轮首次只读连接已见旧cf13a66/PID535、MM/PID1028、secondary自动重启；这些变化在本轮连接前已发生。管理仍usb0，当前卡/线路派生51502、漫游50212，IMS未注册、已有真实420响应，数据关闭、无通话/启用任务。证据 `.local/evidence/ims-profile-production/{baseline.json,runtime-before.json}`。

- secondary日志明确旧unit调用已移除的`secondary-qmi-init`，以INVALIDARGUMENT反复退出（不是已证明的基带故障）。部署将保守停止该无效循环并安装包内canonical `device-init` unit，但不在线运行初始化、不重启MM/基带；普通数据当前关闭。没有为绕过问题增加危险的旧命令别名。
- 生产实现：设备opt-in、标准派生、IMS-only范围；原族循环每次准备独立AT profile；v2持久profile/bearer网络镜像、原owner/SIM、线路代次、设备flock、取消屏蔽、profile清理晚于bearer/网络。普通分支持有同一flock，未知profile不被按APN复用。启动恢复不明则禁止全局namespace搬移。
- 审查补强：每次承载新建接口选择状态但不换MM owner/SIM，代次谓词到达实际CreateBearer/Connect分发；服务退出按SIP/XFRM→bearer→profile顺序回收，持久写入不明保持阻断。旧8秒watchdog改为有界40秒，覆盖5+5+20秒清理预算。
- 注销代码问题已补：从已认证会话生成新nonce-count Digest并保留Security-Verify；Expires0注销不允许因423重建正数租期。历史网络500仍保留，不断言此代码差异是运营商返回500的唯一原因，实机注销仍待验收。
- 初版 `12bd0c7fe81d9b26ebd0d3ea0b918b3ae24c0ca4` Validate已通过；后继 **`16bf1f9309834f97fc649c146df79c08e3cebb48`** 进一步把profile受控setup期限放到外层，内层保留晚到Create结果到profile清理，不因独立90秒超时丢失生命周期衔接。Validate36758599751 / Build36758599884运行中，**尚未宣称最终候选CI、制品或部署通过**。
- 用户历史文档移动保留未提交。一次rustfmt递归触及无关文件的纯格式变化已根据任务起始干净状态恢复，仅保留本任务文件；初轮静态守卫失败后更新为新安全边界并全量重跑通过。Rust只在Actions。
- 私有部署脚本 `.local/active/lan/deploy_production_profiles.py` 分stage/open-window/install，要求最终verified.json，固定SSH主机pin、实际SIM/MM/配置/无通话任务/usb0预检；不复制DB/config、不清预算，候选包更新二进制/前端/设备资源。尚未执行停服或覆盖。

## 先前验收：临时 profile 上实际 IMS IPsec 注册成功（2026-09-30 16:29 UTC）

**本节优先于下方“尚未注册”的历史记录。`83354b620530c4b9dd26fa088a86a6d3e36e849f` 已在当前51502 eSIM、50212漫游网的有界维护探针中实际注册成功；不是主服务持续在线或生产集成完成。**

- 实际路径：独立、动态选择的CID4 `IPV4V6/ims` → MM授予IPv4、实际wwan0 → 2个P-CSCF路由 → 第1候选420 → 第2候选补齐Require/Proxy-Require sec-agree → 401/AKA与Security-Server → 现有REGISTER协议栈返回成功会话，探针`registered=true / registration_mode=ipsec / derived_3gpp_lte_51502`。没有改为固定IPv4或写死CID/接口。
- 420补强仅针对标准派生配置、auth_rounds=0、唯一Unsupported sec-agree以及明确“without sec-agree … is … on”警告词序；保留安全offer/AKA身份，补齐声明而非删安全头。不改catalog/地址族顺序/原大兜底/候选预算。初版b149b70因按原始标点/空格匹配，分支未命中、仍420；83354b6改为匹配已采得的7词序列，回归拒绝off/not/额外扩展/认证后等情况。
- **注销未获网络确认**：探针返回`unregister_result=rejected`，不能说已成功向运营商注销，更不能说现在仍可用/在线。本地承载已释放、namespace库存恢复、bearer receipts为空；仅删除本次自有profile，最终原3项profile/Initial EPS/reporting验证恢复。
- **16:30 UTC最终状态**：MM仍PID48819、当前Modem/5；主服务/beta8/secondary均停止，recovery timer active。正式安装仍cf13a66，配置与DB哈希从写前到清理后相同；没有创建配置/DB备份、没有重启MM/基带或清除恢复预算。此次未拨号、发短信、续期或启动监听。
- `83354b6` 本地228 Python、定向格式/diff通过。Validate **36743092348** / Build **36743092317** 全success，两套实际下载日志核验 **60项累计新增回归+8项兼容检查**均ok，双架构制品digest/meta/ELF/程序/前端核验通过，Publish skipped，旧Release/tag未改。
  ARM64 artifact11110863966，包SHA256 `011c46de075f10d6487c7751fd7e4308c3f2fa05c906184ba53da0fe3a9071fc`，程序SHA256 `2d4a6e4f94ac1685eeef771e1ef8ec0bfd5b6134acf646cb4495a9c6690f19b1`；AMD64 artifact11110853550，包SHA256 `813a20347f67544d7511dfd3b936d76689385d1036c434cd42e61b6928efcf0e`。
- 证据 `.local/evidence/ims-route-completion/83354b6/{verified.json,registration-verified.json,derived-security/}`。被动观察只记录SIP元数据，收到420与带Security-Server/AKA挑战的401；后继IPsec内的注册成功依据程序实际成功会话报告，不伪称AF_PACKET看到了加密内200报文。

### 仍未完成：主服务生产集成与持续在线验收

1. **不能直接将维护CLI接到自动重试**：它要求程序停止、无任何MM bearer；`drain_bearers()`调用全局shutdown，生产进程不能调用。需派生/MM/QCM410专用运行时能力，保留profile持久账本、原MM bus、不可变SIM/线路代次、设备级排他以及取消中的晚到结果。
2. 自有profile生命周期必须关联到pending/active bearer，等待原对象和网络清理完成后再恢复reporting/删除profile；MM对象换代只能重绑定profile清理，不能把旧bearer操作转发到新对象。未知结果保留并阻断，不能让普通APN复用误采纳遗留自有profile。
3. 普通数据共存尚未验收：维护快照拒绝任何MM bearer；应先限定无冲突的IMS-only场景，不为准入而停止数据/secondary。新探针wwan0与旧主服务wwan2有占用环境差异，不能宣称独立profile是唯一因果。
4. 当前成功来自IPv4v6独立profile，无需据此增加“SIP失败后另建IPv4”的新循环；地址族和profile来源大兜底继续保持原顺序。生产覆盖部署/主服务注册/自然续期仍待后继，不能把暂存探针当成已部署主程序。
5. 注销被拒绝需独立分析（保留该事实）；不要为了得到好看的结论自动重放REGISTER/注销、重启MM或清预算。用户的历史文档移动仍保留未提交。

## 路由补全续接：2026-09-30（以下为此前逐步记录）

已重新核对六份交接、Git、GitHub Actions、旧抓包脚本与实际调用链。**当前安装的是 `cf13a666c59d1194401bf4d4229f820ac55f448d`，主服务已由用户停止；此前运行PID3365。** 包含a269e9d路由补全及后继旧对象清理修复；两套CI/双架构、部署、路由和旧receipt自动结案均已验证，但本项目版本未注册成功。用户新提供的beta8同机成功对照见下节。

**最新进展：用户刚在同一设备手工运行 `/root/temp/simadmin`（beta8）并确认蜂窝IMS注册成功，随后已停止beta8，且没有重新启动SimAdmin主服务。10:56 UTC只读核实两程序均无运行进程，`simadmin.service` inactive；安装目录仍是cf13a66但不是正在运行的版本。** 不根据下方旧PID3365记录擅自启动服务。MM当前PID48819、secondary服务inactive，这些变化发生在用户测试前后，不是agent重启/停服；管理仍走usb0。

### 有界 REGISTER 续接（已收到420；派生安全声明补全待验证）

已读取本轮 JSONL 最后未完成代码并续接；不要把下述候选当成已部署修复。

- 初版 `f65b474f008cf0276e5aca059e2d683cfaf9e4f3` 增加显式 `probe`，复用现有派生身份/AKA/REGISTER，独立 namespace/内存数据库，不启动主服务或自动恢复。持久化一次性探针状态，限制一次底层承载；报告注销结果和清理结果，不把曾注册成功说成仍在线。
- 补上审查发现的 owner 交接竞态：临时 profile 的原 `MmBus` 直接传到底层建承载，不重新获取 well-known owner；普通生产路径仍按原逻辑发现 MM。清理检查内存与磁盘 bearer receipt，并在 profile release 前阻断遗留探针 namespace。
- 同 owner 对象重新枚举的 profile 清理要求旧对象消失、稳定 SIM/slot、物理控制口、原 profile/EPS 与双快照一致。旧 receipt 缺少新增稳定身份字段时保守保留，不自动迁移或删除解锁。
- 后继 **`1381e25623760ef9b45d4ce327e443e8d2d84e15`** 修复复审发现的 reporting 写入竞态：启用 reporting 也经原 unique-owner bus、串行锁内 SIM/静止检查并读回，不再通过探针核心里的 mmcli 可复用选择器写入。结果不明或取消后 `Probing` 不允许自动 release；不会清账本来绕过。
- 本地 Linux Python **227 项通过**、Windows同227项通过（其中28个POSIX项按平台skip）；定向格式/diff通过。一次Linux全量检查限时中断仅到15项，后来完整重跑通过，记录均保留。Rust仍只在Actions，最新 Validate `36735762535`、Build `36735762412` 运行中，**尚未宣称双套回归或制品通过**；较早f65b474不用于设备试验。
- **15:00 UTC只读现场复核**：主服务/beta8/secondary均停止，MM PID48819、Modem/0，bearer/call为空，DB ok、启用任务0，管理usb0；正式安装仍cf13a66/hash未变。没有新建profile、创建承载、REGISTER或重启MM。证明 `.local/evidence/ims-route-completion/probe-resume-readonly.json`。
- **1381e25两套CI/双架构已全部核验**：Validate36735762535/Build36735762412全success，两份日志56累计新增回归+8兼容检查逐项ok，制品digest/meta/ELF/程序/前端摘要通过，Publish skipped。证明 `.local/evidence/ims-route-completion/1381e25/verified.json`。
- **15:29–15:31 UTC首轮实机闭环**：创建独立IPv4v6/ims CID4并启用reporting，但探针在承载创建前返回 `ims_access_registration_parked`。这是独立维护线路缺少coordinator准入初始化，**没有创建承载或发送REGISTER，不是网络拒绝**。随后release恢复reporting、仅删自有profile；最终3项、pending=null、完整token与试验前一致，config.yaml/data.db哈希未变，原namespace列表未变、无bearer receipts，MM48819及服务状态未变。证据 `1381e25/dual-first/`。
- 后继 **`8c0e0e6cac91c24789df4e8ffb3ae8e87c7b7e39`** 为独立探针线路在transition lock内发布cellular-only准入；普通生产线路仍默认parked、WLAN不获准。Linux228 Python通过，Validate36737616859/Build36737616747全success；两份日志57累计新增+8兼容均ok，双架构制品完整核验通过，Publish skipped。
- **8c0e0e6实机已到SIP**：三个独立有界窗口`dual-first`、`dual-metadata`、`dual-warning`均使用临时IPv4v6/ims，动态CID4、MM实际授予IPv4、实际接口wwan0、精确AT关联2个P-CSCF并预装路由；每窗口4个初始候选均收到420，auth_rounds=0，未注册。后两窗口只增加被动元数据观测，不改请求字段；不把三个窗口说成一次REGISTER。
- 被动证据明确`Unsupported: sec-agree`、Warning399；第三窗口按词白名单保存的警告词序是`without sec-agree and <other> is <other> on`，未保存SIP包体/原始警告/身份。代码当前发送Security-Client但这些候选均无Require/Proxy-Require声明；后继只拟在**标准派生配置+420+唯一Unsupported sec-agree+此明确缺失声明警告+auth0**时，补齐既有安全声明，不删安全头、不扩预算、不改catalog/外层兜底。尚未把假设写成已修复。
- 每窗口均已验证承载/namespace回收并release临时profile，恢复原3项/原EPS/reporting，配置/DB摘要未变，服务仍停止、MM48819未重启。MM对象因接口归还而换代，release的同owner重绑定通过；因此跨窗口完整token因modem/SIM对象路径变化而不同，**不声称token始终相同**。
- 第三窗口首次release在PRE阶段看到MM对象列表为空而拒绝，未分发任何写；该失败记录已单独归档，待重新枚举完成后重新预检并成功release。没有删除未知receipt/预算或重启MM。证据 `.local/evidence/ims-route-completion/8c0e0e6/{verified.json,dual-first/,dual-metadata/,dual-warning/}`。
- 注意实际wwan0与旧主服务wwan2不同，都是MM动态选择且经过拓扑/独占核验；当前隔离窗口与旧主服务的数据承载占用不同，不能把“独立profile”认定为唯一因果，也不能写死网口。生产profile生命周期尚未集成。
- 当前待办：核验后继两套CI/双架构，再执行独立profile有界对照（仍先保持IPv4v6），安全回收后才决定后继；生产派生侧集成仍未完成。私有脚本 `.local/active/lan/bounded_profile_register.py` 每个动作保留独立记录，不能绕过 `verified.json` 或覆盖失败记录重放。
- 用户移动历史 `ESIM_IMS_PROFILE_TEST_2026-09-01.md` 的工作区修改保留未提交。

### Exact-family 显式维护入口（已完成CI与profile闭环，未注册对照）

用户已批准派生侧临时自有IMS profile生命周期和一次受控对照，并确认现有CID自动选择不应重做。已检查MM1.24源码：该QMI驱动IndexField=profile-id，Set不传ID走Create Profile，传ID走Modify；因此新增**显式维护命令**而非改变自动注册循环。

- `mm-ims-profile-lease` 默认inspect，acquire需要匹配当前快照的plan token，release要求APN/family匹配自有记录。严格新建并接受实际返回ID，验证唯一tag/MM与AT族/APN、完整原profile/EPS/reporting未变。
- 需要两套程序停止、无bearer/call/未知承载receipt；不创建承载、不启动服务、不修改现有PDP、Initial EPS、默认族顺序或profile大兜底。元数据账本在/var/lib持久保存，Set/恢复reporting/Delete结果不明时保留并阻断，不盲重试写操作。
- `c71bee2`与`b83a0f4`两套CI/双架构均通过；设备QMI Set创建请求均返回明确`invalid-parameter-length`，原3项profile/完整快照token未变。44字节tag缩至16仍失败，**不是已证实名称长度根因**。首版明确拒绝记录经MM日志/tag/owner/原快照验证后归档保留；新版Rejected由工具核验后结案，未删任何profile或预算。
- **最终维护候选 `6a77d9278e5d8fbaf4519fa0c77dfc2a6a5c7835`** 增加显式`acquire-at`：通过原MM owner的Command读取能力/活动、动态选MM与AT列表中均不存在的CID、写前重读、写后核验。不是direct-QMI，不会自动从QMI失败退到AT，不改变生产CID选择和大兜底。
- 本地**219 Python**、格式/diff通过；Validate [`36722549817`](https://github.com/autisticryptic/SimMaster/actions/runs/36722549817)、Build [`36722549812`](https://github.com/autisticryptic/SimMaster/actions/runs/36722549812)全success，实际下载日志核实**44累计新增/更新回归+8兼容检查**，双架构制品核验通过，Publish skipped。
  ARM64 artifact11101166888，包SHA-256 `99fb2b31f2b1d742473be503820a8322ea66218eef9fa180098d3c61b9170c86`，程序SHA-256 `d801b34fc7e7b6e817b6d91e3e319cb9dc0a7194d846d06b14df40b4a8756687`；AMD64 artifact11101677001，包SHA-256 `968e453ce637d6e43842b0130db83759a6f640bc598d5f32fce92962b5f21436`。
- **13:43 UTC profile闭环实机成功**：经inspect token准入，MM-AT自动选CID4创建独立`IPV4V6/ims`，MM/AT读回且原条目、InitialEPS/reporting保持；随后只删除本次自有profile，最终inspect回到原3项、pending为空、完整token与最初相同。没有修改CID1/2/3，**没有创建承载或发送REGISTER**。
- 主服务/beta8/secondary仍停止，MM PID48819未变，正式安装仍cf13a66；维护候选只放在独立staging运行，没有覆盖安装或创建配置/DB备份。recovery timer在写操作期间暂停并恢复；其service为已成功结束的`oneshot/RemainAfterExit=yes/active-exited/MainPID0`，一次预检误拒绝未发写，记录保留。
- 证明`.local/evidence/ims-route-completion/6a77d92/verified.json`与`profile-{acquire-at,release,inspect}.json`；前两版拒绝及归档证明保留。设计/命令/约束：[MM exact-family profile维护](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)。
- **未完成：临时profile上的有界注册对照及生产派生侧集成。** 需要先把MM对象换代后的profile归属与清理衔接做好，不能直接把本工具接到无限重试，也不能把“profile创建成功”说成“当前eSIM注册成功”。

### beta8 同机成功对照（最新，优先于下方无成功对照的记录）

- `/root/temp/simadmin` ELF ARM64、8732248字节、SHA-256 `210c35b11f54dd240a83e90dd08d5e8a8f4f2cea227ce3a0503a9ced4140f9b7`，meta为**1.1.7-beta8 / 930365d**，与本地既有二进制分析完全一致，不是只看名称猜版本。
- 用户明确确认注册成功。beta8终端输出未作为本次日志保存，不能伪造捕获了SIP200；MM日志独立证明其IMS承载有实际收发：profile-id4/APN ims的IPv4 Bearer/2持续约31秒，TX4434/RX1463，最后收到用户Disconnect。
- MM请求时序明确：**profile4 + ip type ipv6**的Bearer/1连续三次被`ggsn-reject`拒绝，随后新建**profile4 + ip type ipv4**的Bearer/2成功；这不是从实际IPv4地址反推请求类型。当前项目此前使用profile3/IPV4V6，MM会按profile自身族决定实际WDS路径。
- 当前网络仍50212；停止后AT+CGDCONT与MM ProfileManager只读列表均只见1/2占位以及3=`IPV4V6/ims`，profile4已不存在。结合相同哈希二进制的既有分析，beta8使用临时独立IMS profile并随尝试族准备定义；**尚无这次profile4完整定义/网口/namespace/SIP参数的运行中快照**，不把数字4当成修复方法。
- **对照并非只差版本**：系统日志显示测试前主服务停止、secondary停止、MM重启；beta8成功时MM已是新PID48819。agent本轮只读，未重放这些操作。不能将成功直接证明为某个单一profile字段的因果，也不能声称必须重启MM或停secondary。
- 当前代码的确定性缺口：`prepare_ims_profile_context`仅按APN复用一次profile，随后所有family请求携带相同pin；MM1.24加载pin配置后以其PDP族为准。更重要的是dual bearer拿到IPv4便被视为成功，后面的SIP无响应**不会回到同一个bearer循环执行IPv6/IPv4**；所以只改preferred CID或单族请求标签不能复现beta8路径。
- 下一步需要独立、可回收的exact-family IMS profile对照，保持既有profile3/普通PDP/Initial EPS/预算不变，不写死4，不借用别人CID或切换direct-QMI。生产修复须明确派生侧准备边界、同owner/SIM/generation账本和失败/取消/删除验证；详见[exact-family设计](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)。**这不只是调整REGISTER字段，尚未新建profile、改变地址族设置或启动任一程序。**
- 本地证据`.local/evidence/ims-route-completion/beta8-success/`：`stopped-snapshot.json`、`selected-config-and-mm.json`、`mm-bearer-metadata.json`、`profile-command-evidence.json`、`post-run-definitions.json`、`current-mm-profiles.json`。未提交凭据、数据库、真实SIM身份或原始日志。

### 继续排查：WDS绑定与包完整性（2026-09-30，最新）

用户已确认手机成功是**非VoWiFi蜂窝IMS**，但不记得当时漫游网。接入类型已确认，不能继续把这个问题当待回复阻塞；仍未证明手机与测试机同在50212网络。

- 09:03及10:40 UTC核实仍为cf13a66/PID3365、MM587/secondary346，管理usb0，DB正常、无活动通话/启用任务。09:03曾停止于`emm-invalid-state`；10:40在既有自动流程的REGISTER阶段、未注册。未修改profile/库/普通PDP、Initial EPS、USB或预算；MM/基带未重启。
- **120秒释放因果已缩小**：boot-relative日志显示，MM先收到WDS `Packet Service Status: disconnected` / 3GPP原因36 `regular-deactivation`，约9秒后应用才清理网卡并导致MM重新探测。不是应用先在120秒执行Disconnect所造成；这仍不证明运营商主动拒绝，也可能来自基带内部。
- 在用户要求继续修复范围内，通过原API**只POST一次retry（HTTP202）**进行有界采证，未清任何持久预算。临时将MM日志设DEBUG，远端210秒守卫自动恢复INFO，之后又显式确认恢复INFO；没有重启服务或直接QMI/AT激活/绑定。
  注意“一次”是一次API批次，不是一条REGISTER：原自动校准会继续建立后续承载，不能隐瞒成只有一次底层尝试。后续仅被动采样，没有再次POST。
- **已取得真实QMI绑定请求/响应**：IMS WDS IPv4 client4、IPv6 client5均请求`Bind Data Port: a2-mux-rmnet2`，对应事务返回SUCCESS；与已核验wwan2/dev_port2一致。此前“没有实际绑定应答”的缺口已补足，不再据此猜测应换网口。
- **WDS统计与Linux出向观测不一致**：对应IPv4 client在REGISTER开始前已有TX328，约30秒变1312后不再增长、RX0，随后120秒结束。不能把这1312字节算成已发送的几十个SIP包，也不能凭计数命名认定空口已送达。该差异尚需低层证据解释。
- **报文完整性被动核验**：另一次100秒只读AF_PACKET观测捕获32个出向UDP5060、0入向；IP/UDP校验和全部正确。同期wwan2 qdisc发送从16包/17639字节增长到49包/63614字节，drop/requeue/backlog均0。只保留地址/长度/校验/消息类别等元数据，不保存SIP身份或包体。
- 既有MM DEBUG记录还显示重新创建modem时WDA从802.3调整为raw-ip，Set/Get均成功、QoS=no、aggregation=disabled；后续IMS仍无应答。**不能把初始化前的802.3单独当成已证实持续格式错误**，也未手工修改WDA。
- 只读确认CID3仍`IPV4V6/ims`；未改定义。当前内核`CONFIG_FTRACE`、`CONFIG_KPROBES`均未启用，tracefs无现成追踪入口，不能用现有内核进一步证明DMA/基带内部收发；没有为此刷内核、重启或安装工具。
- **当前结论仍是未注册，未有证据支持再改域名、7200秒有效期或强制sec-agree就能解决。** 下一步需要同50212漫游手机成功对照/脱敏注册证据，或另行安排能观察驱动/DMA/基带的诊断窗口；不在既有限制下擅自改内核、跨CID/换网口试错或迁移native。
- 本地证据在`.local/evidence/ims-route-completion/cf13a66/`：`release-timeline.json`、`wds-single-observation.json`、`wds-message-summary.json`、`passive-data-format.json`、`passive-packet-integrity.json`、`kernel-observation-support.json`、`profile-definitions-readonly.json`。
  首轮元数据提取遇到journal的null MESSAGE而失败，日志级别已恢复；仅重读既有日志补齐证据，未重放retry。该次tcpdump输出未成功保存，**不把它记为新抓包通过**；32包完整性结论来自后来独立的被动AF_PACKET记录。

### cf13a66 最新实机验收与下一步

- 08:15重连看到旧a269e9d/PID489，MM587/secondary346，数据库ok、无通话/启用任务。PID变化发生在重连前，非agent操作。1条旧receipt由安装器只读证明对象和网络已消失，**安装器未删除它**。
- 08:19:03 UTC新程序cf13a66/PID3365运行，二进制SHA-256 `786c35352d4832aae291ec7dd3096a4493c30a8dbcba71ab125c1f6615b64847`匹配已验证ARM64；复制窗口config.yaml/data.db哈希未变，无备份，MM587/secondary346未重启，timer恢复active，预算目录未被清除。
- **旧对象清理修复已实机通过**：新程序自行结案旧记录；之后一次IMS承载结束/Modem再次换代时，日志再次出现`Retired old MM IMS ownership record after object and network absence verification`，校准后成功在Modem/2创建Bearer/8。新连接不再被旧Modem路径的receipt永久堵住；原owner/SIM/网络安全边界保留。
- **IMS注册仍未成功**：受控只读观察窗口内，两条动态P-CSCF路由均预装，UE抓包44个wwan2 Out UDP5060、0个入向；初始REGISTER和强制sec-agree候选均无完整响应/AKA轮数0，MM约120秒后结束IMS承载，仍记录tx1312/rx0。没有触发POST retry或修改库/profile。
- 08:28:32 UTC再次核验：PID3365/运行hash仍匹配，前端MD5 `01505b0195870511cc8428e1d730b53c`与制品一致，DB quick_check=ok，MM587/secondary346、timer active。
  证据`.local/evidence/ims-route-completion/cf13a66/{deployment-second,registration-observation,installed-verified}.json`；不要用a269e9d旧采样代替此轮。
- 本次08:28快照时尚缺手机接入类型和QMI绑定应答；**上面的继续排查已确认蜂窝IMS、补齐WDS绑定成功应答，并记录有界日志级别调整**。仍不能凭标准域名/库ready断言运营商必然接受LTE，不用VoWiFi ePDG当蜂窝P-CSCF。设备无strace，未安装工具或发direct-QMI绑定命令。
- 本地Bash已恢复，213 Python/文档检查重跑通过。最终目标“当前eSIM像手机一样注册”仍未完成；真实换卡/故障注入、长通话续期、Pixel受控A/B、MM网口移回导致重新探测等长期项仍保持未验收。

### 后续旧对象清理补强：代码/CI及部署已完成（下列连接失败为历史）

- 06:23 UTC只读复核仍运行a269e9d/PID3002，MM598/secondary353未变，管理usb0、DB正常、无通话/启用任务。IMS exhausted，错误引用Modem/0，当前为Modem/1；1条receipt保留。
- 代码追踪纠正“新连接缓存旧modem路径”的猜测：QCM410 transport是无字段provider，新连接会重读绑定；真正失败在Create前的`recover_owned()`，它先重试旧`OwnedLease.bus`上的清理RPC。旧owner仍存在使该检查通过，但旧modem与bearer已消失，UnknownMethod导致永远无法结案。
- 已只读看到旧modem/bearer均不存在，wwan2已回主机且地址为0、原namespace无wwan2。现补充程序化保守验证：原owner ObjectManager确认双对象缺失→只读验证原网口/地址/源路由/私有规则/namespace无残留→再次确认对象缺失，才结案。未知/失败/owner变更仍阻断；不删除预算或其他资源、不把UnknownMethod直接当owner丢失。
- **最终候选 `cf13a666c59d1194401bf4d4229f820ac55f448d` 已推送并验证**。213 Python、定向格式/diff通过；Validate [`36680824631`](https://github.com/autisticryptic/SimMaster/actions/runs/36680824631) / Rust job109775661618、Build [`36680824551`](https://github.com/autisticryptic/SimMaster/actions/runs/36680824551) / Rust job109776078962、两架构全部success，Publish skipped。
  下载两套日志逐项核实**27个累计新增/更新回归名（前轮20+本轮7）及8兼容检查**均ok，包括实际private-D-Bus双对象消失/UnknownMethod不能推断为空、网络残留/对象回归/owner变化保护。
- 初版39baf20两套CI编译失败E0283，已在6f7b190显式指定String错误类型修复；随后根据实机`ip -j -N rule`仍将table编码为数字字符串的事实补回归，最终为cf13a66。失败annotations/记录保留，不以早期SHA代替最终候选。
- 两包已实际下载核验官方artifact digest、`1.1.5/cf13a66`、ELF与程序/前端摘要：ARM64 artifact11081323457，包SHA-256 `60647bc441d390f8b808096aa7f35d42712360ba8416b5c0d7814cf40cf6a46a`，程序SHA-256 `786c35352d4832aae291ec7dd3096a4493c30a8dbcba71ab125c1f6615b64847`；AMD64 artifact11081328485，包SHA-256 `aa08fd9afb625a7455386f24fc0ef0ec07869ba6c2c577992a524e782c42da17`。
  证明`.local/evidence/ims-route-completion/cf13a66/verified.json`；首次下载途中DNS临时失败，重试后全部核验通过，旧Release/tag未动。
- 部署前已只读独立证明旧receipt的原owner/双对象缺失和主机/namespace网络无残留，**未删除receipt**。后继安装脚本只允许保留这种已验证记录给新程序再次核验并结案，不跳过未知资源检查。设备busctl对嵌套MM变体JSON输出报`Failed to create new json object`，这是工具编码错误，已改用完整typed输出保守核验，不当成空对象。
- **07:14:37 UTC后继部署未执行**：SSH连接TimeoutError，记录只有`phase=preflight`，未登录/上传/停服/改文件；Windows当前无192.168.68.*地址，访问192.168.68.1走WLAN默认网关。已询问用户确认设备/RNDIS或新IP，停止重复连接。
  已审阅的后继脚本`.local/active/lan/deploy_retired_lease_candidate.py`；连接恢复后仍须重新核对实际运行hash/SIM/服务/通话/任务/管理链路/receipt，不能直接重放。
- **首次REGISTER无响应仍未解决**，不能归因于这个后续清理缺陷，也不盲套7200秒/required配置。已询问用户：手机成功是否在关闭Wi-Fi、同样50212漫游下走蜂窝IMS，还是VoWiFi；尚待确认。
- 另已读取ModemManager1.24.0公开源码：qcom-soc按`dev_port`选择A2_MUX_RMNET0…7，并执行WDS Bind Data Port。实机wwan2 dev_port2与该映射一致，尚无实际QMI应答逐消息证明错误，不擅自改用其他通道。

### 第二次部署成功与实测结果（优先于下方中断记录）

- 05:58:59 UTC重连：旧828135b/PID490已运行，MM598/secondary353，管理usb0，DB正常、无通话/启用任务、receipt/create均0；PID变化先于本轮操作。曾见`emm-invalid-state`，未靠重启MM/基带清除。
- **06:00:28 UTC覆盖部署成功：a269e9d / PID3002**，运行SHA-256 `160cc0da071aab549c2c66ed564dcb704f0716f18e42becf8258234d96edf761`匹配已验证ARM64。无备份，复制窗口config.yaml/data.db校验未变；MM598/secondary353未重启，timer已恢复active，恢复预算未删除。
  使用独立暂存目录`ims-routes-a269e9d-second`，保留首次失败证据；成功记录`.local/evidence/ims-route-completion/a269e9d/deployment-second.json`。
- **全候选路由已实机证明**：同一承载CID3精确AT归属得到2个P-CSCF，日志`candidate_count=2 routed_count=2`早于首REGISTER；命名空间两条目标路由均走实际`wwan2`。抓到**44个wwan2 Out UDP5060包**，未见入向SIP响应，AKA轮数0，尚未注册。
- **不能再用tx_packets=0断言没有发包**：该机bam-dmux所有网口计数为0，但MM普通数据承载有收发统计；仓库`netdev.rs::send_probe`本已有此驱动不更新计数的说明。只读检查wwan2 `dev_port=2`、同bam-dmux父级，未见runtime-PM error；Linux6.17-rc6参考驱动也没有更新这些统计。AF_PACKET出向抓包证明内核交给该接口，不等于基带/运营商已经收到。
- 对应IMS承载约124秒后结束，MM报告tx1312/rx0（不能等同于44个SIP包已上空口）；网卡移回主机再次引发Modem/0→Modem/1。程序记录binding recalibrated后，后续尝试仍有旧Modem/0错误和保守保留receipt。**实际WDS/SIO绑定尚无逐消息证据，不猜测或用direct-QMI bind-mux试错。**
- 设备时钟比采证主机慢约89分钟，首次按主机UTC筛选journal为空；已补采本次boot的monotonic日志，以PID/承载/相对时序关联，未改设备时间。证据`registration-observation.json`、`datapath-readonly.json`、`channel-mapping.json`；未发送POST retry、通话或短信。
- **用户要求的数据库对照已完成**：见[51502连接配置参考](IMS_CATALOG_PIXEL_IOS_COMPARISON_2026-09-29.md#8-当前-esim51502-的连接配置参考2026-09-30)。三库确有Globe/51502参数：iPhone LTE ready，IPCC LTE unsupported（volte=false），Pixel LTE/NR ready。
  APN/domain/身份/PCO发现/ipv4v6基本与派生一致；iPhone明确required安全策略与7200秒请求有效期，三库有显式VoWiFi ePDG `weconnect.globe.com.ph`。静态值不证明当前无响应原因；iPhone Contact投影缺口、VoWiFi roaming/缺IDi边界均已标明。未安装库/改profile，源库SHA不变。

### 第一次临时 IP 上线与部署中断（历史，第二次已成功）

- 用户提供 `192.168.68.1` 后，使用既有 **root 密码认证**成功连接；原设备主机公钥 pin 匹配。pin 只验证服务器身份，不要求用户配置公钥登录。凭据只在仓库外，未回显/提交。
- 05:40:37 UTC 新现场：`828135b/PID482`，运行 hash 仍匹配旧验证制品；MM590/secondary341，Modem/0，数据库 ok、无通话/启用任务，receipt/create 当时均 0；管理实际走 `usb0`，不是旧 wlan0。这些 PID 变化在本轮连接前已发生，不是 agent 重启。
- 用户明确允许沿 `usb0` 部署后，再次预检并上传已验证 ARM64 包到 `/opt/simadmin-staging/ims-routes-a269e9d`。**激活失败：`deployment_phase=verify_owned_cleanup / exit_code=1`。**
  脚本已停止 `simadmin.service`，随后发现清理后的持久 ownership 记录未归零而保守退出；**未进入 `overwrite_verified_files`，没有覆盖旧828135b程序、前端、配置或数据库**。没有创建备份、删除receipt/预算、重启MM/secondary或拨号。`simadmin-modem-recovery.timer` 已由 finally 恢复active。
- 用户随即报告 RNDIS 使笔记本失去互联网，并明确为此把设备离线。此后只读检查了笔记本：当前 RNDIS 网卡已不在列表，IPv4互联网默认路径为WLAN；**未更改Windows网卡/路由/DNS，也未继续设备请求**。
  已建议后续让RNDIS只保留同网段管理地址、无默认网关/DNS，Wi-Fi保持上网，不改设备USB模式。
- **不能把这次操作称为部署成功。** 最后已知旧主服务被本脚本停止，离线后状态未知；设备上线第一步核实主服务/运行hash/MM与SIM/receipt和实际网络残留，处理服务可用性。不要直接重放脚本（暂存目录已存在），更不能删除未知receipt来绕过检查。
  本地证据 `.local/evidence/ims-route-completion/a269e9d/deployment.json`、`reviewed-activation.sh`；脚本 `.local/active/lan/deploy_route_candidate.py` 只作为已执行范式审阅。

- 03:32 UTC 固定公钥只读核实 LAN 目标仍为 `828135b / PID85175`，运行 hash 与既有已验证 ARM64 一致；MM577 / secondary343 未变，管理走 `wlan0`，数据库 `quick_check=ok`，无活动通话、无启用任务、未安装 catalog。
  IMS `registered=false / recovery_state=exhausted`，错误引用已不存在的 `Modem/1`，现存对象为 `Modem/2`；有 1 条 receipt、无 `.create`，**没有删除或重放恢复**。
- **纠正上一轮根因断言**：`capture.json` 的单条路由、零包/零接口计数是真实记录，但脚本 `route get` 使用写死的历史地址，不是本次承载动态候选；快照主要在首候选阶段。源码对每个实际候选先 await 路由安装，再创建绑定该接口的 socket，失败会传播；日志另有 WDS `tx=1312 bytes`。因此旧证据不能证明实际 REGISTER 经 veth 发出，也不能认定路由是零包的唯一根因。
- 蜂窝实现：在第一条 REGISTER 前为当前承载已接受的 P-CSCF 及本轮实际发现结果逐项预装路由；按 IP 去重，每个地址独立失败，未成功准备者不能进入 SIP；保留发送前幂等路由检查和代次/承载校验。不改变 PCO→精确归属 AT→配置/DNS 的既有发现优先级，不扩大 CID 归属规则。
- VoWiFi 实际链路：DNS 已保留 A/AAAA 列表，IPv4 veth 默认路由本来覆盖所有 IPv4 目标；真正的消费截断是完整握手只取前 5 个地址。现改为完整遍历、成功即止，浅状态探针仍只取 1 个，profile/内层地址族/proposal/path 顺序与限制保持。
  另发现 SOCKS5 原来只用于 DNS，live IKE 仍无条件直连；现通过同一捕获的 UE worker 创建 TCP 控制与 UDP relay，外层 socket 按实际 relay 族绑定，每个 IKE/NAT-T/ESP 数据报携带自己的最终地址/端口，不因代理失败偷偷直连。
- 保留边界：没有为测试机开启 VoWiFi；未新增主机/UE IPv6 上游配置，缺少外层 IPv6 网络不能靠虚构路由解决。既有代理主机名解析与 DNS 系统回退路径未迁移；不能宣传为所有 DNS 都在 worker 或完全无本地 DNS。UDP relay 私有模式及域名型 SOCKS relay 地址仍明确不支持；本次未改 NAT-T 源端口策略。
- 本地已通过 209 Python、21 前端 unit、TypeScript / ESLint、定向 rustfmt / diff；Rust 只交给 Actions。新增回归覆盖多候选、路由失败/代次、超过五个 ePDG、SOCKS 不同目标/族/端口、控制连接生命周期、8 KiB 包与超时。
  证据 `.local/evidence/ims-route-completion/`；Rust 验证结果如下，实机注册仍待设备上线。

### 候选已验证，部署因用户确认离线暂停

- 代码已通过 Windows `git.exe` 推送 `simmaster/master`：**`a269e9d6f7c5b359e142ae7734598c009f091914`**。
- Validate [`36671791336`](https://github.com/autisticryptic/SimMaster/actions/runs/36671791336) / Rust job `109748160357`、Build [`36671791369`](https://github.com/autisticryptic/SimMaster/actions/runs/36671791369) / Rust job `109748379048` 全 success；前端和 ARM64/AMD64 构建通过，Publish Release skipped。
- 两套日志 artifact 均已下载并校验官方 SHA-256，逐项确认 **20 个本候选新增/更新回归名 + 8 个重点兼容检查为 ok**，不是只看绿色 workflow。
- ARM64 artifact `11078356878`，包 SHA-256 `a7b270fa97fd53529ec88f0d92578153883a213a19eed2ea9aa685d549fffae6`；二进制 SHA-256 `160cc0da071aab549c2c66ed564dcb704f0716f18e42becf8258234d96edf761`。
  AMD64 artifact `11078283114`，包 SHA-256 `a0637d577c50b11a8178e78780f2c3f418593dfd52559c8f288f7307aa8ae565`。
  两包均实际验证 `1.1.5/a269e9d`、ELF 架构、二进制与前端校验；完整证明 `.local/evidence/ims-route-completion/a269e9d/verified.json`。
- **另一处旧文档事实已校正**：GitHub 现有 `v1.1.5` Release ID `398850006` / tag 指向 `09edc038f3110f23bce43dcbd77747955e0b1fa7`，发布于 **2026-09-29 05:34:39 UTC（本会话前）**，并非旧文档记载的16998ae。
  核验脚本首次因旧 tag 断言拒绝收尾，重新读取发布日期/tag/资产时间确认后才更新核验基线；没有移动/恢复 tag，也没有覆盖 Release。保留首次核验失败记录和 `release-observed.json`。
- 03:32 UTC 的 `828135b/PID85175` 是**最后可达实机证据**。之后两次固定公钥 SSH 在 TCP 阶段超时，Windows `192.168.100.13:22` 也超时；05:17:27 UTC 有界复核仍超时。
  用户随后明确“设备暂时离线，回头更新临时 IP”，因此停止后续连接。**未上传制品、未停止任何服务、未改配置/数据库、未动 receipt/预算、未触发注册重试或呼叫。**
- **收到新 IP 后继续**：安全更新仓库外目标凭据索引，沿用原 SSH 主机公钥 pin（不同则停止核验，不自动接受）；重新检查当前运行 SHA、SIM/MM owner、无通话/任务、数据库、`wlan0` 和遗留 receipt。
  `.local/active/lan/route_completion_readonly.py` 与 `route_inventory_remote.py` 是已审阅的只读参考；旧部署/抓包脚本含旧 PID/哈希/删 receipt/POST，不直接重放。receipt 归属与残留网络未核实时不得删除解锁。
  使用上述已验证 ARM64 包覆盖，不建备份，保留配置与数据库，临时停再恢复 recovery timer，不重启 MM/secondary；随后用**当次动态 P-CSCF、源地址、实际网口与同一承载窗口**核对路由/计数/包与 SIP 响应。若仍不出包，再只读核查 BAM-DMUX/WDS 对应关系。

## 本轮交接：2026-09-30（历史快照，以上续接结论优先）

**下一位 AI 的完整入口是 [NEXT_AI_HANDOFF_2026-09-30.md](NEXT_AI_HANDOFF_2026-09-30.md)**，
已包含当前实机状态、已批准但尚未实现的路由补全，以及用户提过但未进 todo 的全部遗留项。要点：

- 设备为局域网测试机 `192.168.100.13`（QCM410，aarch64，root SSH，管理走 `wlan0`）；凭据只在仓库外，不要提交或回显。
- 已部署 `1.1.5 / 828135b`（含 MM 数据接口修正），主进程 PID `85175`；该机全局配置曾丢失、现为默认值，
  未安装 `carrier-bundles.sqlite3`，profile 回落派生 `derived_3gpp_lte_51502`；IMS **尚未注册**。
- **当前最高优先级**：为本承载实际下发的**每个** P-CSCF 地址都补上走 IMS 承载的路由；
  VoWiFi 的 ePDG 域名多地址做同类补全。已实机证实根因是**本机路由漏装**而非运营商配置：
  IMS 承载已绑定 `wwan2` 且有地址，但 REGISTER 未从该网卡发出（`tx_packets=0`，两侧抓包为 0）。用户已批准该修正。
- 用户约束：只允许在派生配置一侧补强，不改动现有大的兜底逻辑；若确实无法通过派生配置注册，必须直接说明原因。
- 其余未完成项详见新文档 §5（通话中自然续期是否会断话的实机验证、Pixel 库呼入进语音信箱的受控 A/B、
  IMS 网卡移回主机导致 MM 重新枚举并短暂中断普通数据、真实换卡与自动重新附着故障注入验收；
  Android 移植仅作参考、不在本项目实现）。

## 上一轮：2026-09-29 10:37 UTC（UI 与离线边界，已被上文取代）

用户明确设备与eSIM卡暂时下线，正在用手机测试IMS兼容性。**停止SSH/API连接、轮询、部署、切卡、拨号和短信操作**，直到用户确认上线。

三项UI代码、本地检查及整批Actions已完成，候选 **`33d16f3d78ea0456682a2469810625efe29805ec`** 已推送，**设备离线、未部署**。记录在 [NEXT_AI_HANDOFF §0.14](NEXT_AI_HANDOFF_2026-09-28.md#014-用户新增ui待办与设备离线边界2026-09-29-0824-utc)：
1. 概述线路控制的飞行模式移除冗长说明，与另外三个控件显示形式一致。
2. IMS与Trunk页移除额外“IMS注册模式”选择。**只有VoWiFi与蜂窝IMS两个开关都开启，才允许尝试双注册；两路都注册成功且网络协商允许，才算双注册成功。** 仅开一项只用该项，不擅自打开另一项；双注册不成立则在已启用/可用的接入中VoWiFi→4G/5G IMS回退，保留原会话及资费保护。
3. eSIM自动检测后的profile列表将“可用”改成可实际执行的“切换”按钮，复用现有授权、维护锁和绑定校准，不再仅展示或强制跳完整配置管理。

199 Python、21前端unit、TypeScript、lint、定向格式检查通过。本机Vite限时构建未完成的事实保留；**正式前端构建已在Actions通过**。
最新 Validate `36555415147`、Build `36555415060`、Frontend `36555414932` 全success；下载日志逐项核实3新增Rust回归+6重点兼容回归通过，两架构制品已下载核验commit/架构/digest/二进制及前端校验。
证据 `.local/evidence/ui-offline/33d16f3/verified.json`；Publish Release skipped。当前已部署的LAN目标仍以07:14核验的7c6cf86为准，不拿候选当在机版本。
设备上线后再确认部署/实机切换窗口；原历史文档 `docs/archive/2026-09/ESIM_IMS_PROFILE_TEST_2026-09-01.md` 的非本轮未提交修改保留未动。

本地数据库静态对比已完成：[Pixel/iOS/IPCC分析报告](IMS_CATALOG_PIXEL_IOS_COMPARISON_2026-09-29.md)。六份源库哈希保持不变；三组对应新旧config_json无差异。
报告区分历史空Contact/MMTEL缺陷（已修）、来源回退与现存字段投影缺口；没有宣称用户原来那次呼入失败的唯一根因。
现场结论等用户手机测试补充，不把手机结果当成本项目实机已验收。

## LAN部署完成与数据库分析：2026-09-29 07:14 UTC

用户要求先部署新局域网目标，再研究 Pixel 与 iOS/IPCC 外置库差异。详见
[NEXT_AI_HANDOFF §0.13](NEXT_AI_HANDOFF_2026-09-28.md#013-新局域网目标部署及外置数据库对比2026-09-29-0643-utc)。

- 用户授权“直接重新安装”后，已在LAN目标重建 `/opt/simadmin` 并运行 **1.1.5 / 7c6cf86 / PID54742**。
  运行中SHA-256 `2051c398b58c28ffeea396240b7b29614bac67267807bd123963d7c37d12a829` 与已验证ARM64一致；前端校验/HTTP200、服务active/NRestarts0通过。
- 原目录被删除、旧PID470持有deleted DB的问题已处理：先用SQLite共享读锁保全数据库到唯一正式路径，再停旧服务；恢复前后全部表指纹相同，quick_check=ok。原账号/线路/任务/历史保留；全局配置按授权采用默认值，没有重置密码、没有创建备份。
- MM516/secondary339未重启，恢复timer已恢复active。证据 `.local/evidence/lan-deploy/{reinstall-result,installed-verified}.json`。
- **已按顺序开始外置数据库对比**：先核实本地文件来源/版本，再比较Pixel与iOS/IPCC配置；不切换设备配置或拨号。
  新LAN目标不能与旧Cloudflare实验机a6a327e混淆；未登录LAN受保护API，不宣称其IMS/通话验收已完成。

## 结果分类候选验证：2026-09-29 04:44 UTC

用户要求继续剩余todos。最新逐步记录在 [NEXT_AI_HANDOFF §0.12](NEXT_AI_HANDOFF_2026-09-28.md#012-剩余任务续接2026-09-29最新步骤)。

- **结果分类候选 `7c6cf8635971f738a8af3a79a01f50ba6af4e641` 的代码/CI/双架构核验完成，尚未部署**：typed report、当前接入真实180/远端初始INVITE终局证据、local/re-INVITE错误隔离、观察丢失与取消保护、成功详情展示。
- 本地195 Python、13前端unit/type-check/lint、定向格式/diff通过；新结果语义不改写历史失败记录。首版098ae04有E0004编译失败，已修复并保留失败证据，没有跳过门禁。
- 最新 Validate [`36522353436`](https://github.com/autisticryptic/SimMaster/actions/runs/36522353436) / Build [`36522353454`](https://github.com/autisticryptic/SimMaster/actions/runs/36522353454) 全success；两套日志实际核实 **18新回归+4重点兼容检查**均ok，ARM64/AMD64制品实际下载核对官方digest、meta版本/commit/架构、ELF与二进制/前端摘要。
  证据 `.local/evidence/dial-outcomes/7c6cf86/verified.json`；Publish Release skipped。
- 设备最后验证仍为a6a327e，本次没有部署、再次拨号、切卡或故障注入。
- MM换卡/自动恢复故障注入的维护窗口与切换方式已询问用户，待明确确认，不为完成待办打断已注册会话。

## 上阶段收尾：2026-09-28 16:52 UTC

**用户已确认测试来电送达，要求完成文档后阶段性结束。本轮不再操作设备。请优先完整阅读
[NEXT_AI_HANDOFF_2026-09-28.md §0](NEXT_AI_HANDOFF_2026-09-28.md#0-最新续接定时拨号失败与可选非漫游呼叫准入)，尤其 §0.0 的逐步记录。**
该节取代同文件上午 §1–§9 的过期状态，包含逐步验证、用户确认、阶段收尾、保留边界和后续任务。

**16:52:07 UTC 最终只读核验：** 实际主程序仍是 `a6a327e / PID808982`，二进制哈希与ARM64制品一致；
IMS `registered / ipsec / derived_3gpp_lte_46011`，本版本新注册时间 **16:27:54 UTC**，`last_error=null`，home、活动通话0，测试任务disabled。
证据 `.local/evidence/automation-dial/a6a327e/stage-closeout.json`。因此“最新含 CID 自动校准补强的版本可以实际注册 IMS”已有实机证据；
**不等于真实换卡后的自动校准全链路已验收**。本版本自然续期计数目前0，不借用旧dd8ba1f的续期证据。

本阶段完成：代码/CI/双架构核验、部署、新版本IMS注册、按用户标准的测试来电送达。
保留后续：§0.11的拨号任务结果分类、真实换卡/恢复故障注入、接通后音频/时长及原长期事项；不笼统宣称所有模块没有问题。

- **新增优先任务**：定时拨号立即 failed 的诊断，以及可选“已注册 VoWiFi / 明确已驻网非漫游蜂窝”语音准入。
  用户要求保留原限制开关，关闭后仍能主动允许漫游接打电话。旧严格模式默认不变，短信保护不变。
- **最新设备程序：`1.1.5 / a6a327e1246576c57655ff7c3d615ec4289e18aa`**，两套 CI 与双架构通过，**16:22 UTC 已按用户“现在执行”授权覆盖部署**。
  PID808982，运行中 SHA-256 `a50eba8dcc00fbcb266fb0dfc240a49786d956ea4df254b79e0da5fb0343726f` 与 ARM64制品一致；无备份，配置/数据库复制前后校验未变，MM410/secondary283未重启。
  已补齐 pending 取消/owner/重复请求边界、VoWiFi 快速路径、配置兼容、MM fresh-home/unique-owner checked call 和前端错误反馈。
  定时任务只操作自己创建的 IMS call ID，不再把同号码既有 modem 呼叫或 CLCC index 当成自有资源；取消后保留任务完成精确挂机请求。真实接通/音频/挂机仍需实机验收。
- 最新本地检查：**192 Python 全通过、11 前端 unit / type-check / lint 通过、定向 rustfmt / diff 通过**；
  12:38 静态守卫失败与格式 diff 已解决。最新 SHA 的 **28 项新增 Rust/mock/private-D-Bus + 4 项重点兼容测试**在两套日志中均逐项核实为 ok；Rust 只在 Actions 执行。
  Validate [`36446861013`](https://github.com/autisticryptic/SimMaster/actions/runs/36446861013)（Rust job `109011304514`）、
  Build [`36446860858`](https://github.com/autisticryptic/SimMaster/actions/runs/36446860858)（Rust `109011983233`、AMD64 `109011983037`、ARM64 `109011983099`）全绿。
  两架构包已实际下载核对官方 artifact digest、`1.1.5/a6a327e` metadata、ELF、二进制/前端校验；完整证据 `.local/evidence/automation-dial/a6a327e/verified.json`。
  Publish Release skipped；16:03 UTC 再次核对既有 Release/tag 仍为 `16998ae`。每步记录见上述文档 §0.0。
- **11:51–11:56 UTC 实机采样**：经固定 SSH 公钥的 Cloudflare Tunnel 连接成功，设备仍运行 `dd8ba1f`，
  `/proc/511308/exe` 哈希与已验证 ARM64 一致；IMS IPsec 已注册、续期计数 4、最近续期 11:14:31 UTC，calls 列表为空。
  没有部署、改资费配置、创建任务、主动拨号、发短信或切卡。
- 设备 **automation.tasks=[]、dial_call 日志为空**，只找到 09:32:42 UTC 历史 call 触发记录。
  `trunk.enabled=false / trunk.vowifi_only=true / vowifi.enabled=false` 是当前阻断条件，不是历史根因已复现的证明。
  该限制以前是刻意的资费保护，不应直接删除；用户新授权通过可选模式实现。
- 目标号码仅在 `.local/evidence/automation-dial/requested-task.json`，不提交原始号码；授权/执行状态已更新到该私有文件。最新一次测试结果见下方，不重复触发。
- **最新授权与实际呼叫**：用户明确要求现在拨号、任务时间可自行设置，以“用户接到电话”为成功。
  已设置并读回 `vowifi_only=true / allow_home_cellular_calls=true`，其他线路配置/短信未变。
  任务 `task-voice-acceptance-a6a327e` 已创建：目标与用户给定号码匹配，60秒、默认disabled（每日04:00仅作可编辑时间表）。
  **16:31:57 UTC 只立即触发一次**：预检 home/IMS注册/calls=0；设备 dialing→ringing，16:32:29终局 SIP408/Q85031，任务failed、未接通。
  **用户已确认看到了北京时间00:32的未接电话，只是错过接听**。按用户“收到电话即成功”的约定，**来电送达验收通过**；未接听，不代表音频或接通后时长验收通过。
  **16:34:32 calls=[]，没有重拨**，任务保持disabled。现有failed/408历史不改写。证据 `.local/evidence/automation-dial/a6a327e/{deployment,task-configured,immediate-trigger,call-observations,post-call}.json`，用户确认见本轮会话及私有requested-task.json。
- **上阶段新增、现已完成代码/CI的需求**：自定义/计划拨号的对端未接/超时受限成功分类，保留SIP/Q850和未接听事实，不把本端故障或所有408都成功化。
  详见 [NEXT_AI_HANDOFF §0.11](NEXT_AI_HANDOFF_2026-09-28.md#011-拨号任务成功与对端接听结果代码ci完成尚未部署)。设备a6a327e仍使用旧失败分类；只有部署7c6cf86后新语义才生效。
- 下面 `dc2355c` 校准候选的 CI/制品证明依旧有效，已部署语音程序包含其代码；真实换卡与自动重新附着故障注入仍独立待验收。

## 上一阶段已验证进展：2026-09-28 11:41 UTC

本轮已按用户要求恢复实现；历史交接快照保留在
[接手说明与可复制 Prompt](NEXT_AI_HANDOFF_2026-09-28.md)，以下记录优先于该快照。

- **已部署实测基线仍为 `1.1.5 / dd8ba1f`**：SIM-06 IPsec 注册及两次自然续期通过；本轮没有连接或部署设备。
- 校准首版 **`9a8fe726c55669d789bfed9586153689a3b4f42f`** 的 Validate `36411614418`、Build `36411614307`
  已核实均 success，含 Rust/D-Bus 测试步骤、ARM64/AMD64 构建；Release 按 push 门禁 skipped。
- 已保留并完成原 5 文件补强的进一步审查：MM 持久 reporting/profile 不再经旧 CID 清理；单卡 slot=0/缺失归一为 1，
  非法槽值仍拒绝；嵌套 live/恢复批次更新携带不可变 generation，未知库存不消耗 profile 重试预算。
- 新增补强包括：全线路先同步失效再异步 reconcile；MM discovery 失败不伪造 absence；派生 SIM 在 Create 前及 SIP 前后核验；
  终止性绑定错误贯穿设置读取/清理错误；恢复 reporting 在取得串行锁后重验 SIM/槽位/策略；未知网络清理保留 receipt；
  Create 前持久化 intent，歧义结果或 lease+Delete 失败保持阻断，不丢弃晚到的 Create 回复。
- **最新已验证校准候选：`1.1.5 / dc2355c095f08c075f4d6b334c3f5982a9cfc609`**，已推送 `simmaster/master`。
  本地 **188 项 Python、定向 rustfmt、diff 检查通过**；两套 Actions、14 项新增 Rust/mock/D-Bus 回归及双架构制品已核验，详见下节。
  后续仅文档提交不改变这个二进制候选 SHA，不用文档 HEAD 或旧 Release 代替制品 commit。
- 自动校准整体仍未实机验收；真实切卡/外部 eSIM 切换及自动重新附着故障注入须用户另行确认维护窗口。
- 设计与限制：[MM SIM/承载绑定校准](IMS_MM_SIM_BINDING_CALIBRATION.md)。

### 校准候选的代码/CI 验证（未部署）

- [Validate `36416118351`](https://github.com/autisticryptic/SimMaster/actions/runs/36416118351)：success；
  Rust 回归 job `108907556481` 实际完成编译、硬件无关测试和隔离 D-Bus 测试。
- [Build `36416117791`](https://github.com/autisticryptic/SimMaster/actions/runs/36416117791)：success；
  Rust 回归 job `108907860151`、ARM64 job `108907860170`、AMD64 job `108907860149` 均 success。
  前端构建/测试也通过；Publish Release 按 push 门禁 skipped。
- 实际下载两套测试日志并核对 **14 个新增测试名均为 `ok`**，不是仅查看 workflow 标题。
  覆盖 slot=0/非法槽、未知库存暂停、旧任务状态与 AKA 拒绝、IP 读取期间换卡、串行锁内 reporting 准入、
  终止性错误/族循环、owner 丢失 receipt 与 Create intent 保留。测试日志 artifact digest 同样已校验。
- 两架构包均实际下载：GitHub 官方 artifact SHA-256、包内 `1.1.5 / dc2355c`、ELF 架构、二进制与前端校验均匹配。

| 架构 | Artifact ID | 包 SHA-256 |
|---|---|---|
| ARM64 | `10967856151` | `cdf7ab74683778f8a59795e242f61294f5e98332b45dfbb66261e2a4851e9a25` |
| AMD64 | `10968306519` | `40bd4932f136d5b4197fb3b5ce0d271b512191448092142d4cc786eea96285d1` |

完整 artifact digest、二进制哈希、metadata、run/job/step 与逐项测试证据：
`.local/evidence/mm-cid-calibration/resume/dc2355c/verified.json` 及同目录 `jobs-*`、`tests-*`、`release-unchanged.json`。
再次只读核实：Release `397300521 / v1.1.5` 仍指向 `16998ae3c5172890075b2392ded3ce9c711d72b8`；未覆盖、未移 tag。

**下一步必须先确认维护窗口：** 当次核实设备状态、无通话及管理链路后，才可按用户批准覆盖部署候选（不创建备份，
保留配置/数据库/历史证据），随后分别验收正常注册/自然续期、用户安排的 eSIM/实体换卡、外部切换。
自动重新附着故障分支需独立故障注入授权及可用的一次性预算；预算已消耗时不清除、不靠应用重启重置。
未进行上述实机步骤，不能宣称自动校准或自动重新附着实机验收完成。未知 `.create`/network receipt 仍需独立人工核验，不自动删除解锁。

以下既有 SIM-06 修复/发布记录保留作已验证基线，不能与新校准候选混同。

## 1. 当前优先级

1. **版本/发布已完成**：源码、tag、发布包统一为 **1.1.5 / `16998ae`**，用户手动触发的
   Build-Release `36253740079` 已全绿，GitHub `/releases/latest` 已为 `v1.1.5`。
   两架构包实际下载后确认 SHA-256、包内版本/commit 与 ELF 架构一致，不是只改 Release 标题。
2. **SIM-06 中国电信 IMS 注册失败**：用户已确认上线并授权修复、提交 GitHub、部署新的 **1.1.5 构建**。
   2026-09-27/28 已通过固定公钥 SSH 与只读 API 重连；当前已覆盖为 **`1.1.5 / 02dfdc5`**，
   MM 默认后端。经本次明确授权的一次 reporting/重新附着窗口，**06:32:29 UTC 已实际注册成功（IPsec）**。
   有两个 P-CSCF，实际 profile 仍为标准 derived；前两槽认证失败，第三槽轮换 P-CSCF 后成功。
   **09:49 UTC 续接核验：设备实际已运行 `1.1.5 / dd8ba1f`，IPsec 已注册并完成两次自然续期。**
   下述 `02dfdc5` 为历史维护基线，不是当前运行版本。通用恢复及取消安全补强的 CI、制品和部署
   已核验；故意制造 P-CSCF 缺失的自动重新附着实机分支仍未验收，不打断健康会话来强测。
   当前 CID 1=`ctlte`、CID 2=`ctwap` 保持原样；CID 3=`IPV4V6/ims`。
   IPv6-only 对照无效且被用户指出会影响原兜底，已撤销，**不得用固定 IPv6 替代原地址族策略**。
   新现场与部署进展见 [IMS 诊断 §9](IMS_DIAGNOSTICS.md#9-sim-06-现场与-cid-修复2026-09-2728)。
3. native 真机及其他长期任务按 [开发总计划](DEVELOPMENT_PLAN.md) 和
   [后端路线图](MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md) 单独推进，不并行抢占同一设备。

SIM-04 自然续期、SIM-05 手动测试已由用户确认完成，不重复等待或验收；
它们的 MM 结果不代表 native 全能力通过，也不扩大解释 SIM-05 的业务测试覆盖。

## 2. 完成边界：不能称为全项目彻底完成

| 范围 | 状态 |
|---|---|
| IMS 命名/数据库迁移、Hickory DNS、Telegram 反代入口 | 约定代码阶段已完成，有回归/历史 CI；保留旧值兼容与各自实机限制 |
| beta8 与朋友提供的三网源码 | 主要流程及可移植边界有归档分析；不是穷尽所有二进制分支或全部移植 |
| native AT/URC、持久化短信、分片、逐片送达、受控恢复 | 已有代码和 CI；最终功能检查点 `302b70e` |
| native 真机、Quectel/DJI 维护、长稳/代次故障 | 尚未验收，不由 MM 测试替代 |
| 同机不同 modem 混合 MM/native | 尚未实现；目前全局二选一 |
| 未知孤儿资源自动恢复、完全统一跨旧 IMS 的去重/通知恰好一次 | 不在已实现承诺内；未知 receipt 保持阻断 |
| 双注册、多线路、VoWiFi/视频、UT/MWI、E911、CS 音频、1.1.6 | 保留各自实现或实机/发布门槛，不能一并勾选完成 |
| SIM-06 | `dd8ba1f` 已部署，IPsec 注册及两次自然续期通过；自动重新附着故障注入分支仍未实机验收 |

详见 [原生后端当前状态](NATIVE_BACKEND_STATUS.md)。旧总计划存在日期较早的条目，
逐项以最新代码和证据核对，不机械地把所有旧复选框重开或清空。

## 3. 只保留一个开发目录

- 唯一工作区为 **`SimAdmin` / `master`**，GitHub remote 为 `simmaster`（`autisticryptic/SimMaster`）。
  `origin` 是历史本地 remote，不要误推；实际 HEAD 以 `git rev-parse HEAD` 为准。
- 整理前 HEAD：`19c3c122afdf6f053d02b1f1a9a2e3b26a7595bd`。
- 原 `SimAdmin-1.1.5` 不是较新代码，而是 `586985e` 的 detached 快照，落后 12 个提交，
  完整历史已在 master；确认无独有源码、只有 Python 缓存和依赖后已移除该 worktree。
- 目录名、版本字符串、发布标题、tag、二进制 commit 是不同概念。不要重建旧目录来“切到 1.1.5”。

## 4. 验证与发布证据

### 1.1.5 已核验发布基线

- 真实源码提交：`16998ae3c5172890075b2392ded3ce9c711d72b8`。
- push 构建 `36252976546`、Validate `36252976632`、Frontend `36252976689` 均 success；
  push 的 Publish Release skipped 是原门禁行为，不是构建失败。
- 用户手动 dispatch 构建 **`36253740079`** 全部 success，含前端、Rust 回归、arm64/amd64 及 Publish Release。
- Release **`397300521 / v1.1.5`** 为非 draft、非 prerelease、latest；tag 指向上述源码 SHA。
  用户删除了旧 Release；不能继续建议操作不存在的 v1.1.7/v1.1.8 Release，旧 tag 与 Release 分开看。
- 2026-09-27 实际下载验证：两个 `meta.json` 均为 `1.1.5 / 16998ae`，target 与 ELF 机器类型匹配。

| 包 | SHA-256（与发布的 SHA256SUMS 一致） |
|---|---|
| amd64 | `34920b6d0760bc8b16463a736c8e20513457c01e0c75ada0b39e043747abc92f` |
| arm64 | `eba1704d6b760142b8a724547781bb4baa40be5df90cd8b822cb0ed555a5d491` |

- 本地检查为 **169 项 Python 测试通过**，含 28 项采证回归；Rust 仅在 Actions 编译/测试。
- 可核验记录在 `.local/evidence/release-1.1.5/github-verified.json` 与 `artifacts-verified.json`。
- 后续 docs-only 提交可晚于发布 tag；不要把新的文档 HEAD 当作已发布二进制的 commit。

### 历史与未验收边界

`302b70e` 的旧 CI 和早期接手快照仍保留，但不再代替上述真实 1.1.5 证明。
本次发布核验和首轮静态对照（`c601a09`）未部署设备、未改变 IMS 注册算法、未改 MM 默认。
后继 Security-Server 列表修补见下节，不能与先前发布源码混同。
此前对其他提交号、已完成身份错误码补丁或“native 全部验收”的口述不能当证据；
以 Git、明确的 CI run、实际下载包和当前源码为准。

### 后继安全列表修补：代码/CI 已完成，已随候选部署，未覆盖旧 Release

`dfda6cd2ed51e6ee450752a7b565cdb97d4b7906` 补齐多行/逗号 Security-Server 的候选边界与
完整 Security-Verify 回传，不改变默认客户端算法、MMTEL 或 MM 后端。
新增 17 项 Rust 测试及既有 refresh 接线回归，两套 Actions 已实际运行通过：

- [Validate `36291639402`](https://github.com/autisticryptic/SimMaster/actions/runs/36291639402)：success。
- [Build `36291639395`](https://github.com/autisticryptic/SimMaster/actions/runs/36291639395)：前端、
  Rust 回归、amd64/arm64 musl 编译及打包全部 success，Publish Release 按 push 门禁 skipped。
- 本地 172 项 Python 检查、定向 Rust 格式和 diff 检查通过；未在本地编译 Rust。
- 明细证据：`.local/evidence/sim06/security-agreement-ci.json`，对应提交与 job/step 结果已核对。

具体范围及保留的客户端单候选限制见 [IMS 诊断 §8](IMS_DIAGNOSTICS.md#8-后继候选修补security-server-列表2026-09-27)。
这不是 SIM-06 根因/实机修复结论，**已发布 v1.1.5 仍对应 `16998ae`，不包含此后继修补**。
不能重新对现有 tag 直接发布覆盖资产；若需发行或部署候选，应另行明确版本及目标。

### SIM-06 CID 修复与已授权部署（2026-09-28）

- 初版 `8df57a98b27ec41a532b17d9e3b7607b21b4fe8a` 已推送；Build `36325894646` 与
  Validate `36325894682` 均 success，含 Rust 回归、前端和双架构。未修改 Release 发布门禁。
- 部署前补强：新建前读取 `AT+CGDCONT=?`，只选该 PDP 类型支持的空闲 CID；不新建 CID 1，
  不覆盖已有定义（包括空 APN 占位），检查活动状态、重读定义防止覆盖，并读回新建结果。
  对无法确认的写入不自动重复或删除；新定义保留供后续重用。补强版 `e0ade97` 的 Build
  `36368975285`、Validate `36368975330` 均 success；7 项新 Rust 回归实际执行，本地 173 项通过。
- 设备已明确报告 `IP/IPV6/IPV4V6` 支持 CID 1–16；不是只根据 reporting 表猜测支持范围。
- 部署采用 **Actions 制品的版本 + commit + 官方 artifact SHA-256** 核验，版本仍为 1.1.5。
  公开下载中转只能传输公开制品，必须与 GitHub API digest 一致；不向中转发送设备或 GitHub 凭据。
  现有 Release `v1.1.5 / 16998ae` 不覆盖、不移 tag，不能拿该旧包代替新修复。
- 已于 2026-09-28 部署 `1.1.5 / e0ade97`，实际主进程 SHA-256 为
  `badb454b965a4e68d3e69b7c7ddf1dbaf035fe4eebaea62527ef5189894a6c33`；主服务更新，MM/secondary 未重启。
  实机发现 `mmcli` 的 `response: '…'` 外壳被新校验拒绝，仍未创建 IMS CID，注册未成功；
  已追加外壳归一化和真实格式回归，后继 `02dfdc5` 的 Build `36370931907` 与 Validate `36370931916`
  均通过，9 项新增回归实际执行。已直接覆盖部署，主进程实际 SHA-256 为
  `8041d6860fd5866245517bdf450183a643e82e307df792632711b186358e4784`。
- `02dfdc5` 已实际创建 CID 3、打开 reporting，但 P-CSCF 仍缺失；AT 活动 CID 与 MM profile pin
  的对应关系仍待排查，不能认定“创建 profile 就一定修好”。仅本次新建的 CID 3 曾临时改为 IPv6，
  对照无效；用户指出会干扰原兜底后，已于 2026-09-28 03:23 UTC 明确撤销为 **IPV4V6**。
  当前主服务 PID 307733，仍为 `02dfdc5`；配置 `ipv4v6 → ipv6 → ipv4` 未改，原 CID 1/2 未改。
  固定 MM profile 的 PDP 类型会优先于请求族，后续应修正真实兜底接线，不再用固定 IPv6 绕过。
- **用户最新要求：实验机直接覆盖，不再保留备份。** 本次新建的部署备份已按要求删除；
  后续不再创建备份，不删除既有历史诊断、私密资料或用户数据。仍先确认无通话、管理走 `wlan0`、
  制品与目标一致；不得重启 modem/MM、修改 Initial EPS、NV/USB 或扩大到 SIM-04/05 测试。

## 5. MM 维护结果与通用恢复边界

- 用户再次明确：**本轮只修 ModemManager**；native/直接 AT 硬件控制迁移留后续，不再做 AT/QMI 旁路实验。
- 已完整读取用户指定的 [P-CSCF 对照 §7](archive/2026-09/IMS_PCSCF_BETA8_COMPARISON_2026-09-15.md#7-sim-04-实机结论更新2026-09-20--2026-09-21)。
  该节的较新实测结论是：SIM-04 先启用 reporting，再经 MM 做一次 `Disable → Low Power → Enable`
  重新附着，才取得 P-CSCF 和注册；不是固定 IPv6或临时直接 AT 激活的效果。
- 用户已明确批准本轮的一次 MM reporting/重新附着操作。执行前核验唯一活动上下文的实际 APN、
  与原 MM grant 的关联、唯一自有 bearer、无通话和 Wi-Fi 管理路径；只将已确认上下文的 reporting
  打开，经原 MM owner 执行 Disable/Low Power/Enable。未更改 PDP 定义、Initial EPS 或地址族策略。
- 本地脚本误把 MM 的 REGISTERED 状态 8 写成 `>=9`，因此其等待阶段报告超时；这不是网络未恢复的
  证据。只读日志随后确认驻网恢复，程序取得两个 P-CSCF 并在 06:32:29 UTC 完成 IPsec 注册。
  MM/secondary PID 未变，只有主服务按维护操作重启。证据在 `.local/evidence/sim06/deploy-02dfdc5/`。
- 已实现并通过 CI、部署核验的通用恢复（`7896e05`，取消安全补强 `dd8ba1f`）：只在派生配置最终停于 P-CSCF、普通发现/各 profile 槽位耗尽后考虑；
  严格绑定原 MM owner/lease/SIM/实际 grant，唯一活动且实际 APN 匹配的上下文仅作恢复提示，
  **不放宽现有 P-CSCF 地址归属规则**。无通话/数据/VoWiFi或同 modem 其他线路冲突时，释放原 lease
  后至多一次恢复，再运行原 profile 和地址族顺序。预算持久化到 `/run`，普通重试/应用重启不重置。
  不硬编码 MCC/MNC/APN，不调用 native/direct WDS，不向初始 EPS 或现有 profile 写入新值。
- ZIP 与 6 份生产入口文件的字节已再次核验一致；ZIP 注册走 Python 独立 WDS 路径，不能将其
  固定 IPv6/3gnet 激活直接套入本项目 MM 修复。暂不需要新 IDA 解析；确需具体 beta8 分支时再通知用户开启 MCP。

### 2026-09-28 09:49 UTC 最新验收

- GitHub Build `36392766359`、Validate `36392766357` 均 success，对应完整提交
  `dd8ba1f7314150e10c5ce38cafcb18c8c0cf735c`。两架构制品摘要、包/二进制哈希及 21 项新增 Rust
  回归重新核验通过；本地 179 项 Python 检查通过。
- 当前 `/proc/511308/exe` SHA-256 为
  `3ae12007b981bbeb0efca220365b8701f391ab16009346fa7ad457f072900e4d`，与 ARM64 制品一致。
  MM PID 410、secondary PID 283；本轮仅只读采证，没有重复部署或重启。
- 同线路 API 确认 `registered=true / ipsec`，有效 profile 为 `derived_3gpp_lte_46011`。
  初始注册 07:54:27 UTC；08:44:28、09:34:30 UTC 两次自然续期成功，计数为 2。
  地址族顺序仍为 `ipv4v6 → ipv6 → ipv4`，本次实际 IPv6 不代表固定 IPv6。
- 已完成：设备重连、恢复补丁 CI/制品/部署核验、SIM-06 初始注册及自然续期核验。
  未验收：故障注入触发的自动重新附着全链路及取消分支；不关闭 reporting、不清预算强测。
  API 的 `recovery_source=automatic` 不能单独证明新重新附着分支执行过。
- 证据：`.local/session-review/verified-runtime.json`、`connection-result.txt`、`current-ci.json`、
  `python-tests.log`，及 `.local/evidence/sim06/deploy-dd8ba1f/`。旧 Release 仍未覆盖。

## 6. 本地资料布局

| 路径 | 用途 |
|---|---|
| `.local/active/ims/connect_readonly.py` | 当前只读 Cloudflare/SSH 入口；其依赖及已保存主机公钥 pin 同目录 |
| `scripts/ims-readonly-evidence.sh` | 随源码维护的规范采证脚本 |
| `.github/scripts/test_ims_readonly_evidence.py` | 采证工具回归 |
| `.local/evidence/sim06/` | 脱敏访问记录，不是当前在线状态 |
| `.local/evidence/ci/` | 历史 CI 核验与后续构建证据 |
| `.local/checkpoints/pre-cleanup-2026-09-26/` | 整理前 15 文件快照、补丁和测试记录 |
| `.local/cleanup-2026-09-26/` | 原目录清单、52 份原文档、移动/删除清单 |
| `.local/archive/` | 原 `.codex-*`、`.tmp-*`、`.tmp/`、旧 release 包和会话；保留唯一数据及证据 |

`.local/` 不随 Git 分发，其中历史资料可能含凭据，不公开打包。普通 clone 不包含访问权限。
原根目录三份 carrier SQLite 数据保留原位置；主目录的可用前端依赖和构建资源也保留。
Git 私钥及仓库外私密交接未删除或覆盖。

需要原始用户原话时，才查 `.local/archive/sessions/2026-09-24T.jsonl`，更早参考
`2026-09-19.jsonl`；程序化定位并脱敏，不整段输出 Cookie 或原始工具结果。
历史脚本仅作证据，不批量执行、不自动重建旧 bundle、不恢复旧部署流程。

## 7. 设备上线后的只读第一轮

1. 用户确认上线后，先核对本工作区 Git/diff、规范脚本及 `.local/README.md`。
   审阅只读入口后使用既有 Python 环境；不要运行通用客户端的写操作主程序：

   ```sh
   /root/.cache/simadmin-mm-resume-venv/bin/python .local/active/ims/connect_readonly.py
   ```

2. 私密凭据和 host-key pin 缺失时由用户安全提供；不猜密码、不自动信任新主机、不回显秘密。
   最新固定公钥连接已成功（2026-09-27/28），早期 HTTP 530 / Cloudflare 1033 是过期离线观察。
   新错误仍须重新分类，不能认定 Cookie 永久有效/过期；失败不连续轮询。
3. 先看 `/proc/<MainPID>/exe` 哈希、安装 metadata、服务 PID/start ticks/启动类型。
   采样期间进程稳定不证明历史 journal 全部来自该程序版本。
4. 经既有授权应用登录，另行只 GET `/api/cellular-ims/lines`、该线路详情、`/api/modem/backend`。
   核实实际 `line_id`、SIM 作用域和 MM/native 后端，不把“SIM-06”直接当 API line ID。
5. 获取 `phase/stage/last_error`、连接/候选尝试、恢复和下一重试时间及未解决 receipt。
   按 bearer/IP 族 → P-CSCF → AKA/安全协商 → 真实 SIP 响应定位，归属必须和线路/时段交叉核对。
   只读入口不会自动登录应用、查询这些 API 或认定日志属于 SIM-06。
6. 无证据不猜 APN/身份/运营商特例；明确缺陷后才改代码。部署需当次确认，核验目标、无通话、
   制品 commit/版本/架构/校验和；不根据目录名或 Release 显示名称部署。

采证字段、脱敏和输出边界见 [IMS 只读诊断](IMS_DIAGNOSTICS.md)。

## 8. 不得被旧摘要覆盖的约束

- MM 保持默认；native 必须显式实验 opt-in，不能自动停 MM、接管、写 NV/USB 或重启设备。
- 单一 agent 修改代码/配置/部署，其他 agent 只读；不自动拨号、发短信、改变费用保护。
- 用户取消了测试窗口自动回滚；不恢复固定 120 秒 refresh、旧包激活或续期轮询。
- 重插/重启/节点换代不等于资源消失；未知 receipt 不删除，已释放 CID 不重放。
- `a4a83c2` push 失败、SIM-04/05 等待验收等旧摘要已经过期。
- 持久化 `volte_ims` 已通过迁移归一到 `cellular_ims`，保留旧值读取；不要回退迁移，
  也不机械替换剩余历史名称或真实 VoLTE 语音语义，见 [命名与兼容](IMS_NAMING_MIGRATION.md)。
- Rust 编译/测试/双架构构建只在 Actions。本地允许 Python、格式/语法和文档检查。

## 9. 新对话可直接复制

> 请先读 `docs/HANDOFF.md`，核对当前 Git/diff 和版本修正/CI 状态。
> 唯一开发目录是 SimAdmin/master，旧 SimAdmin-1.1.5 已安全收口，不能当较新分支。
> 若我已确认设备上线，再按文档只读核实 SIM-06 中国电信的实际版本、线路和失败阶段；
> 否则不要连接或轮询设备。不要重做 SIM-04/05 验收，不清理 `.local/` 或私密材料，
> 不把旧 CI、安装 metadata 或历史日志当新程序实测。无现场证据不猜根因，部署需另行确认。

更多文档按 [文档导航](README.md) 查阅；新进展更新本文，不再另建根目录接手副本。
