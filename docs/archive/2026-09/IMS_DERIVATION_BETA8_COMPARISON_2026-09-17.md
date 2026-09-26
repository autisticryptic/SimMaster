# beta8 与当前项目：IMS 派生配置及注册链路对照

> 历史档案：本文件保留对应日期的事实，旧版本、worktree 和操作步骤不代表当前状态。
> 当前接手请读 [HANDOFF](../../HANDOFF.md)，不要重放旧部署、回滚或设备命令。

> 整理日期：2026-09-17。续接材料：本目录 `2026-09-15.jsonl`，实际记录包含 2026-09-16 的后续分析。
>
> **范围**：以蜂窝 IMS/VoLTE 为主，从身份、配置、承载、P-CSCF，到 REGISTER、AKA/IPsec、注册成功和续期完整串联；VoWiFi 仅用于说明不同接入的默认值，本文不是 beta8 的完整 VoWiFi 逆向报告。
>
> **证据边界**：当前项目依据真实源码；beta8 依据同哈希二进制的 IDA、指令、格式常量及已有静态记录。未能可靠还原的细节单列，不用推测填空，也不把静态实现当成实机通过。
>
> **本轮没有部署或运行 beta8，没有连接设备、修改业务配置、重新注册或进行短信/通话测试。** 不移除 MM，不将另一设备的 native 后端迁移混入本专项。

## 1. 先读结论

1. **beta8 确实在实际注册路径中动态派生 IMS 身份和归属域。** 不是仅含 `ims.mnc` 字符串：可追到 IMSI/MNC 长度选择、域格式化、承载与 REGISTER 调用。
2. **“派生身份”不等于“派生一切”。** IMSI 可以参与生成 IMPI/IMPU/realm；P-CSCF 地址、IMS 承载授权、AKA 密钥和漫游服务资格不能由 IMSI 算出来。beta8 仍需 MM/SIM、网络配置和真实 USIM 认证。
3. **当前项目已经有完整派生配置框架，不是缺少基本 3GPP 域或空 AKA Authorization。** 它还具备来源绑定的三槽、字段覆盖、UE 隔离及按网络租期续期。应补具体差异，不宜整体照搬 beta8。
4. **最值得优先检查的是 MM 承载配置的读取与地址族投影。** 当前 QCA410 路径由 MM 创建并持有 bearer，但地址/DNS/P-CSCF 主要来自 MM 代发的 AT 查询；`MmBus::status` 没有读取 `Ip4Config/Ip6Config`。驱动又只启动所给 family 切片的第一族，配置 `ipv4v6` 不等于实际双栈。
5. **beta8 的兼容性选择不都更好。** 三位 MNC 猜测、占位小区信息、MD5-96/null 安全组合、IPsec runtime 出错后的宽泛明文回退，以及固定间隔重入注册都不应无条件移植。当前更严格的身份、防串线、资源归属与受保护续期逻辑应保留。
6. **SIM-04 的历史失败不能直接归因为身份派生错误或运营商未开通漫游。** 有效观察停在 P-CSCF 获取阶段；旧 beta8 试运行又卡在不等价的普通数据 APN 前置分支，没有形成两版同条件注册对照。

## 2. 样本、源码版本与证据约定

### 2.1 样本身份

| 对象 | 本次核对的版本 / 摘要 | 用途 |
| --- | --- | --- |
| beta8 包 metadata | `1.1.7-beta8`，commit `930365d`，构建时间 `2026-07-27T10:47:10+08:00`，目标 `aarch64-unknown-linux-musl` | 历史参考，不是当前项目的后续发布 |
| beta8 可执行文件 | SHA256 `210c35b11f54dd240a83e90dd08d5e8a8f4f2cea227ce3a0503a9ced4140f9b7`；MD5 `d0903ceab475bacaf00e7ef45d1403c5` | 本地文件与本轮 IDA metadata 一致；ELF64、小端、AArch64（e_machine=183） |
| beta8 参考包 | 历史核验 SHA256 `ab943a799421d1759d611be089342ba3427382cd8a0c7a9327b5cd4228854bdd` | 见此前 P-CSCF 对照记录；本轮未重新下载 |
| **P：当前主要项目** | 相邻 `SimAdmin-1.1.5`，分支 `dev/1.1.5-modem-backends`；HEAD `905afdf705db219c01eb4c82144375445f8df6be`，后端源码基线 `269be650066bc73275b6bd58623db13d3eadbf02` | 本文主要源码对照 |
| **O：原 IMS 分支** | 本目录 `SimAdmin`，分支 `fix/sim02-catalog-aka-baseline`；HEAD `4c1a73800239f9f9eca0d670603ce2930db1c872`，后端源码基线 `684e2a71e0227dc66f9b7591ab982485933bde15` | 区分旧代码已具备的能力 |

已核对两组“源码基线 → 文档 HEAD”的 `backend/src` 差异均为空。**HEAD、包版本字符串、设备当前二进制不是同一个概念**；本轮没有确认设备实时版本。

参考包来源：[用户提供的 beta8](https://github.com/lilith-rong/SimAdmin-Enhance/blob/Backup-Vowifi-and-VoLTE/Volte/simadmin_1.1.7-beta8.tar.gz)。摘要标识样本，不等于发行者签名。

### 2.2 如何阅读引用

- **Bxx**：第 9 节 beta8 二进制证据索引。地址统一为 **IDA base=0**。
- **Sxx**：第 10 节源码证据索引。默认根为 **P**；明确写 **O** 才指本目录旧树。
- “已确认”表示已看到相关控制流、调用或字段读取，不表示该分支在某张卡上执行成功。
- “未确认”表示缺少完整数据流/控制流证据，不是断言功能不存在。
- 本地 `annotated-*.txt` **左列**是本文地址；其指令目标、字符串注释常使用 `+0x400000` 映射。例如目标 `0x5A8674` 对应本文 `0x1A8674`，不能混抄。
- Rust 字符串是“指针+长度”，多个字面量可能相邻。必须查格式块起点和切片长度；内部子串没有 xref 不等于没用到。
- `source-reference/` 下的恢复文件是分析产物，不是 beta8 原作者源码；本文不拿它的行号冒充原源码。

### 2.3 三种不同的“派生/回退”

| 概念 | 实际含义 |
| --- | --- |
| 身份派生 | 根据可靠 home PLMN/IMSI 拼域、私有身份和公共身份 |
| profile 派生 | 生成含 APN、REGISTER、接入及安全默认值的标准配置对象 |
| 候选回退 | 某来源缺失/不适用或某轮失败后，执行来源内部派生、后续 profile、地址族或 SIP 策略 |

beta8 的身份派生已确认；**没有充分证据证明它拥有当前项目同构的 database → carrier_catalog → derived 三槽系统**。

## 3. beta8：从派生到注册的主流程

```text
运行入口
  → 读取 MM/SIM 信息、AT+CIMI，必要时 SIM IMSI fallback
  → home operator / EF_AD / 兼容猜测，确定 MNC 长度
  → 派生 home IMS 域、IMPI/IMPU
  → 普通数据 APN 与 IMS 路径分配（两类 APN 不可混同）
  → IMS PDP profile 准备，按路径使用 MM bearer 或自有 WDS
  → 当前上下文 / AT / WDS 取得地址与 P-CSCF
  → 对候选 P-CSCF 安装路由，先进入 IPsec runtime
      初始 REGISTER → 401/407 → AKA/必要的AUTS → 受保护注册/监听/续期
  → 若 IPsec runtime 返回 Err，清理后进入 plain runtime
      初始 REGISTER → 401/407 → AKA → UDP注册/监听/定时重调
  → 正常返回则结束；两个 runtime 均失败则尝试下一 P-CSCF
```

这是功能级主链。所有硬件布局、分支选择和错误回退都相同的说法不成立；需要结合下面的适用范围。

### 3.1 IMSI、MNC 长度与 home 域【B01、B02】

**IMSI**：`0x19014C` 内优先 `AT+CIMI`，经 MM 命令 wrapper 发出；没有有效 IMSI 时退到 SIM 信息。不能据此说 beta8 直接打开 AT tty，也不能把当前项目的 EF_IMSI fallback 自动算作 beta8 已确认能力。

**MNC 长度选择**，在 `0x1915BC–0x191634` 可见：

1. MM home-operator 候选有效、为 5/6 位且与 IMSI 前缀匹配时，取长度减 3。
2. 否则采纳有效 EF_AD 的 2/3 位长度。
3. 再否则走兼容猜测：**MCC 460 取两位 MNC，其他取三位 MNC**。

因此，beta8 不只是固定按前三位 MNC 切分，但最后一级仍是猜测。上游 MM operator 在各种驻网情形下的完整来源选择尚未全部还原，不能仅凭 `home_operator` 标签认定其漫游隔离与当前项目相同。

派生骨架：

```text
home_domain = ims.mnc<MNC左补零到3位>.mcc<MCC>.3gppnetwork.org
IMPI        = <IMSI>@<home_domain>
IMPU        = sip:<IMSI>@<home_domain>
registrar   = sip:<home_domain>     # 所见 REGISTER helper 的请求URI骨架
```

例：已可靠确定 home 为 `45507` 时，域为 `ims.mnc007.mcc455.3gppnetwork.org`；漫游到 `46011` 不应把它改成访问网的 IMS 域。**补零是 DNS 格式，不是改写原 PLMN 的 MNC 长度。**

IMPI/IMPU 模板可由 plain helper 的格式块与调用核对；beta8 是否在所有模式支持独立 realm/身份覆盖、是否读取 ISIM 身份 EF，尚未确认。

### 3.2 EF_AD 的强项与解析限制【B02】

已见 QMI primary provisioning USIM 的 EF_AD 读取，以及备用 `AT+CRSM=176,28589,0,0,4`。其价值是利用 SIM 元数据，而不是只靠国家表猜 MNC。

本轮补读 beta8 **AT CRSM 分支**还确认：

- 找 `+CRSM:` 后的引号 payload，要求长度至少 8 个字符；取 `[6..8]` 做十六进制解析。
- `0x19141C–0x191428` 对整个结果检查是否为 `02/03`，没有先取低半字节；因此该分支不能像当前实现一样接受第 4 字节 `0x82/0x83`。
- 这段未见当前项目的“唯一回复、精确长度、严格 SW1/SW2、前后 IMSI 一致”整套保护；**不把这一局部观察扩张成 beta8 所有 EF_AD 路径都无保护**。

结论：可以学习“多来源获取 MNC 长度”，不应退回猜测或照抄较宽松的解析。

### 3.3 普通数据 APN 与 IMS APN 必须分开【B03】

`0x1A8674` 的早期普通数据路径依次尝试：配置中的 APN 候选 → 中国网络的内置 APN 分支（含 `cmnet/3gnet/ctnet`，`46015→cbnet`）→ 枚举 MM bearer 提取 `apn:`。候选耗尽时才到 `0x1A9700` 的 `volte_data_path_apn_missing`。

**这纠正了旧会话中的过度归因**：该错误不能直接证明“IMS APN 没有派生”，更不能只凭它认定某张当前 SQLite 表格式是唯一根因。它首先说明普通数据路径分配的 APN 前置需求没有满足。getter 的完整字段映射和那次试运行输入仍不足以重建唯一原因。

IMS 侧另外有明确 `apn=ims` 的 bearer/PDP 格式块，`0x1A0A80` 对已租用 CID 按尝试类型准备：

```text
停用目标 PDP context → 定义 <cid>,<本轮PDP类型>,"ims"
                    → 启用目标CID的 P-CSCF 上报 1,1,1
```

这里描述的是静态行为，不是要在当前设备重放这些命令。**“beta8 能派生 IMS 身份”并不消除它自己的旧配置/普通数据路径前置条件。**

### 3.4 PDP profile、MM 与自有 WDS【B04】

- `0x19A008` 查看 PDP 定义/活动情况及占用集合，租用未占用 CID；已有本程序 lease 时可复用。
- `0x1A0A80` 按本轮 IP 类型准备 IMS PDP 定义和 P-CSCF 上报。这与当前项目“复用既有同 APN 定义而不改 type”的行为不同；**差异存在不等于已证明它造成 SIM-04 失败**。
- `0x196634` 读取 `SIMADMIN_MM_IMS_BEARER`。它是 **MM bearer 对象路径覆盖值，不是一个 true/false 后端开关**；所见缺失/空值分支退到 `/org/freedesktop/ModemManager1/Bearer/1`，并查询/按策略重建 bearer。此历史默认对象号不能移植到当前项目。
- 另一路 `0x19D6DC` 分配并保留自己的 WDS client，设置地址族，以 `apn=ims,3gpp-profile=<cid>,ip-type=<family>` 建立网络并查询 current settings。
- 二进制同时包含双栈尝试及退单栈分支。所有配置组合的默认顺序、布局选择与生命周期尚不能概括成一条通用规则。

本轮补齐了承载到注册的调用边：单族 MM 路径 `0x1AA060 → 0x196634`，自持 WDS 路径 `0x1AA0E0 → 0x19D6DC`；结果在 `0x1AA2F8 → 0x3F0BA8` 汇合，后者尾跳到 `0x19B9F4`。双栈结果也使用同一跳板。此前直接搜索 `BL 0x19B9F4` 不足以发现这条尾调用。

**对当前项目的边界**：可以比较输入、授予 family、profile lease 和配置读取结果；不能借“参考 beta8”去借用 MM 内部 WDS CID、绕过 MM 建第二套 bearer owner、接管 DATA6 或破坏 UE 隔离。

### 3.5 P-CSCF 与 DNS：哪里已确认，哪里没有【B05】

已确认的发现能力：

- 主路径可以检查当前活动 `CGCONTRDP`，使用解析到的候选。
- 没有预取结果时，`0x19B9F4` 有 **最多 6 轮、间隔 1 秒**的当前上下文读取。
- `0x19D6DC` 的**自有 WDS client**额外发 Get Current Settings（消息 `0x002d`），请求 mask `0x0c00`，解析返回 TLV `0x23` 的四字节地址列表；对应 libqmi 定义为 IPv4 P-CSCF 地址列表。所见 IPv6 标志分支跳过此段。

不能越过的结论边界：

1. 该 WDS IPv4 查询不是 SIM-04 IPv6 的现成修复，也不授权查询别人的 client。
2. 尚未确认 beta8 有 `pcscf.<home-domain>` / SRV / NAPTR 的完整 DNS 回退链。没有相关字面量只是线索，**不能断言绝无 DNS**；也不能把当前源码的 DNS 策略写成 beta8 强项。
3. 各模式中 AT、预取、MM 和 WDS 的完整优先级，以及 CID/APN/SIM/generation 关联保护，仍有待恢复的细节。
4. AT 没显示 P-CSCF/DNS，不等于网络一定没发送 PCO。反之，仅有 IP 地址也不等于能进行 IMS REGISTER。

### 3.6 初始 REGISTER 与真实 AKA【B06、B07】

plain 注册 helper `0x18D548` 的已确认流程：

1. 构造域与身份、Call-ID，初始 `CSeq=1`、请求 `Expires=3600`。
2. 通过 `0x1AC338` 构造 **空 AKA Authorization**：带 username/realm/URI，nonce/response 为空，`algorithm=AKAv1-MD5`。这是请求挑战，不是认证成功。
3. 初始收到 **423**，最多两次按 `Min-Expires` 增大租期并递增 CSeq；新值必须比旧值大且不超过 **604800 秒**。
4. 随后只接受 **401/407** 进入挑战；nonce 解码不足 32 字节拒绝，不能把普通短 nonce 当 RAND/AUTN。
5. 发现 USIM AID，调用真实 UIM AUTHENTICATE，计算 Digest，再发认证 REGISTER。
6. 所见 plain helper 对认证响应**精确要求 200**；成功后解析/记录接受头并返回 runtime。不能把它描述成任意初始 2xx 直接成功。

UIM 主链本轮补读到完整尾部：

```text
@qmi-proxy → 分配UIM client → 打开USIM逻辑通道
  → AUTHENTICATE(RAND,AUTN)
  → 处理必要的6C/61状态（调整Le/GET RESPONSE）
  → 尝试关闭逻辑通道、释放client
  → SW1/SW2=90/00且非空 → 解析DB成功材料或DC同步失败材料
```

`0x1BDDCC`、连接 helper `0x1BD6C8`、清理 helper `0x1BD500/0x1BD3C0` 有对应代码。**这是 qmi-proxy UIM，不是 MM D-Bus AKA 方法**。清理调用存在不代表每个异常分支、每个清理返回值和密钥生命周期都已审计。

AID 发现失败时 beta8 有内置完整 USIM AID fallback；不应假定它适合所有 SIM。Digest 中可见 AKA-v2 派生及无 qop / `auth` 分支，其他 qop 报错；本文不据不完整类型恢复列出所有算法组合。

### 3.7 IPsec、安全头与业务声明【B08】

beta8 的 IPsec 注册 helper 中已确认 offer：

```text
ipsec-3gpp;prot=esp;mod=trans;spi-c=...;spi-s=...;
port-c=...;port-s=...;alg=hmac-md5-96;ealg=null
```

不只是字符串：`0x198000` 引用该模板，`0x189828` 的 XFRM 安装器也构造 `auth-trunc hmac(md5) ... 96` 与 `enc ecb(cipher_null)`。`null` 不提供机密性保护；“能兼容某网”不等于“安全更强”。

**IPsec helper 的握手顺序**已补读：

1. 初始 UDP/5060 发送含空 AKA 与 Security-Client 的 REGISTER；初始 CSeq=1、Expires=3600。
2. 423 时按 Min-Expires 增大并重发，受 604800 秒上限约束；此 IPsec 循环未见独立次数上限，**不能套用 plain helper 的两次限制**。
3. 非423只接受401/407；先解析 Digest 与 Security-Server，再执行 AKA。缺失/解析失败的 Security-Server 在首次 AKA 之前就可失败。
4. RES 非空进入正常路径；RES 空且 AUTS 存在时，先在初始 UDP socket 发一次 AUTS REGISTER。只接受新的401/407，重新解析挑战及 Security-Server，再做 AKA；第二次 RES 仍空即失败，未见第二轮 AUTS 回边。
5. 同族且 IK 长度16等检查通过后，安装 state/policy，绑定本地发送/接收端口，发送带认证和 Security-Verify 的 REGISTER；认证响应精确200才成功。

**Security-Server 检查的实际范围**：`0x18D1E4` 合并头值后取第一个逗号片段，要求 `spi-c/spi-s` 可解析为u32、`port-c/port-s` 可解析为u16。在已读 parser 和 IPsec 调用链内，未见按机制/prot/mod/alg/ealg与所发 offer 匹配的白名单选择，XFRM算法仍固定为MD5-96/null。存储机制名、记录接受头，不等于完成严格 sec-agree 验证；Digest算法检查也不是ESP算法检查。

**IPsec → plain 的实际触发**：`0x19B9F4` 在路由可用后调用完整 IPsec runtime `0x1A0D14`。返回 Ok 则结束；返回 Err 则记录fallback、清 XFRM、调用 plain runtime `0x1A4DD4`。该外层分支未按具体错误类型筛选；不只是握手失败，后续 IPsec refresh 的 Err 也能到这里。路由失败/无P-CSCF则在更早处分流，不是所有失败都一律进plain。

清理 helper `0x18A490` 的参数表实际为 `ip xfrm policy flush` / `ip xfrm state flush`，不是按当前SPI精确删除。执行时的namespace范围没有在这些片段中完整确认，不能据此断言一定清宿主全局；但也不能把它描述为已证明只清当前session。**宽泛降级和flush都不宜移植到当前项目。**

REGISTER/业务头另有以下边界：

- builder `0x1A43B4` 可按输入加入 Security-Client / Security-Verify 及 sec-agree 头，不是每个REGISTER无条件带齐。
- 所见 Contact 明确包含 SMSIP/接入类型；不能等同当前项目的 MMTEL/audio 声明，更不能从注册推出语音全业务支持。
- 所见 PANI 有“至多五字符前缀 + `0000000`”格式，未证明来自真实服务小区，不能照抄为小区身份。

### 3.8 注册成功与续期【B09】

plain / IPsec runtime 分别记录 registered、监听并处理后续事件。接受头中可见 P-Associated-URI、Contact、Service-Route、Feature-Caps、Security-Verify；**解析/记录了某头，不等于它已在每条后续业务路由中使用**。

两条 runtime 的所见调度均使用 **2700 秒（45 分钟）**；plain 定时路径重新调用 `0x18D548`，该 helper 又初始化空 AKA、CSeq=1、Expires=3600。因此不能把 beta8 的 `refreshed` 日志直接等同于当前项目“保留认证上下文并增量刷新原会话”的实现。

尚未确认它在全部模式中依据最终 Contact 租期动态重算期限。已确认 IPsec refresh 返回 Err 可触发上节的 plain 回退；其他运行错误、旧 SA/socket 保留与 supervisor 完整退避仍未全部审计。固定 2700 秒不应当作通用网络租期算法移植。

## 4. 当前项目 P：对应链路怎样工作

### 4.1 管理边界：MM owner 与数据来源是两件事【S01、S06、S08】

当前默认后端仍是 MM；native 是另一条显式启用路径，不是失败后自动接管。

| 工作 | 当前 QCA410/MM 主路径 |
| --- | --- |
| 创建/连接/删除 IMS bearer | typed D-Bus `CreateBearer/Connect/Disconnect/DeleteBearer`，固定 MM unique owner |
| modem/SIM 属性 | MM / mmcli 查询 |
| CIMI、CRSM、PDP/P-CSCF 观察 | `mmcli --command` 由 MM 代发 AT，不直接打开 tty |
| UICC 应用发现 | `qmicli --device-open-proxy --uim-get-card-status` |
| USIM EF_IMSI/EF_AD、AKA | 应用经 `@qmi-proxy` 发送 UIM/APDU，不冒充 D-Bus API |
| 数据接口、SIP、IPsec | 应用拥有私有 bearer 数据接口，迁入线路 UE worker；SIP socket / XFRM 在对应隔离域中 |

`native_bearer.rs`、`ApplicationOwnedNative`、日志“Native VoLTE”是历史/抽象命名，**不能据此说当前已绕过 MM**。准确表述是“MM 管理承载，应用使用受控观察与 SIM 认证接口”，不是第二套 WDS owner，也不是纯 D-Bus 全链。

### 4.2 身份与来源选择【S02、S03、S04】

**IMSI**：MM 代发 CIMI → MM SIM IMSI → 同 slot/完整 USIM AID 的 UIM EF_IMSI。先发现 slot 内完整 AID，避免使用另一 slot 的身份材料。

**home/MNC**：

- SIM home operator 优先，必须 5/6 位且匹配 IMSI；serving PLMN 仅在明确 `registration-state=home` 时可补充，漫游访问网不决定归属域。
- 缺失时取同 IMSI 的 EF_AD 第四字节低半字节，只接受 2/3。
- 再缺时，在总共 12 秒预算内做 `CIMI → CRSM EF_AD → CIMI`，前后身份一致才接受；CRSM 回复唯一、SW=90/00、payload 精确四字节。
- 仍不明确可用有效 custom/catalog 元数据推断，但 5/6 位边界必须一致；歧义不能按最长前缀或三位默认猜。
- 公网标准派生拒绝 MCC999 私网。

**profile 三槽**：默认 database → carrier_catalog → derived，保存值可重排、重复；成功即停止后续槽。显式 ID 绑定来源，不在别的库找同名替代品；库缺失/不适用时可能在**同一槽**解析为 derived。catalog LTE `unknown` 不能擅自改成 `ready`。

**覆盖与身份**：该 SIM 的 `ims_cellular` override 叠加 domain、realm、registrar、P-CSCF、APN 等；实际 IMPI 用 `IMSI@effective.realm`，IMPU 用 `sip:IMSI@effective.domain`。realm/domain 不一定相同。检测到 ISIM AID 不表示此链读取 ISIM EF_IMPI/IMPU 取代标准派生。

应记录 `requested_source`、`effective_source`、profile ID、槽 index、fallback reason，而不是只看 UI 保存的顺序。derived 首槽、来源内部 fallback、后续槽恢复分别验收。

### 4.3 当前 standard-derived 的默认值【S04】

| 项目 | LTE/EPC 派生默认 |
| --- | --- |
| profile ID | `derived_3gpp_lte_<原始PLMN>`，标明 unverified standard fallback |
| APN / 域 | `ims` / 标准 home IMS 域；realm 默认同域，仍可有效覆盖 |
| P-CSCF | 不硬编码运营商 IP，等待网络/配置/发现 |
| SIP 主实现 | UDP，默认本地端口 5060，请求租期 3600 秒 |
| 初始 Authorization | `aka_empty`；VoWiFi 默认仍为 `none`，不能混用接入基线 |
| 业务声明 | 默认 MMTEL/audio，是否可用仍需网络和业务验收 |
| PANI / visited network | LTE PANI `dynamic_if_known`；不默认附访问网身份，不伪造小区 |
| sec-agree | `auto`，不默认强制 Require/Proxy-Require；disabled 必须保持禁用 |
| LTE Security-Client | `hmac-sha-1-96/aes-cbc/esp/trans` |
| VoWiFi 区别 | 另有允许 SHA1-96/null 的机制集合；不能套成 LTE 默认 |

profile 中有 `ip_stack=ipv4v6`、transport 等字段，**不表示所有字段已经决定当前 LTE live 行为**。线路 family 列表才是该链的计划输入，当前 `connect_family` 仍明确用 UDP；配置里有 TCP 不等于该路径已实现 TCP 切换。

### 4.4 PDP 准备与实际 family 的落差【S05、S06】

当前先读 PDP 定义：

1. 有同 APN context：优先指定 CID，否则最小 CID，**不改写已有 PDP type**。
2. 没有匹配项：仅 preferred CID 未定义且确认 inactive 时才新定义；占用、坏行、重复/歧义状态拒绝写。
3. 准备失败会告警并允许继续 APN-only MM 请求，不能把“拒绝写 profile”误述为整个连接立刻终止。
4. 成功时设置该 CID 的 P-CSCF 上报；由 MM 激活 bearer，不在此链用 AT 激活。

**实际 family**：通用计划支持 `ipv4v6 → ipv6 → ipv4`，但 QCA410 `establish_bearer` 只取 `families.first()`；MM create properties 仅映射 IPv4→flag1、IPv6→flag2，没有 dual flag4。因此默认首项落到 IPv6 单族，某些失败后还可能再尝试列表中的 IPv6。此行为 **O 已有**，不能当成 P 新引入的回归。

应把请求 family、下发 MM ip-type、实际授予 family 分开记录；不能仅凭 UI `ipv4v6` 认定双栈通过。

### 4.5 MM bearer、配置读取与 UE 隔离【S06】

- 只创建新的私有 MM bearer；绑定 system bus ID / MM unique owner，不复用别人已连接的接口。
- **Connect 前**写 OwnedLease，记录对象、进程、接口和后续 namespace/network 归属，退出/崩溃恢复只清理自己的资源。
- `MmBus::status` 只读 Connected、Interface、Properties.apn；**没有 Ip4Config/Ip6Config 读取**。
- 驱动最多 12 次读取请求的精确 CID/APN 的 `CGCONTRDP`，必要时由 `CGPADDR` 证明紧凑双族布局中的本地地址。地址/网关/DNS/P-CSCF 再构成 `ImsBearerInfo`。这里的“精确查询”不等于已从 MM 属性证明该 CID 就是新建 bearer 实际使用的 PDP 编号；APN-only 等路径仍需核对这种映射。
- 未实际启动的另一族全部过滤，避免误用固件残留字段。
- 仅数据 netdev 进入绑定 generation 的 UE worker；主 QMI 控制节点不移动，DATA6 不作为 IMS 后门。配置地址及 DNS/P-CSCF 主机路由，不接管宿主管理默认路由。

项目另有 MM key-value bearer parser，但其存在不能证明当前 QCA410 live 已用 D-Bus IP 配置。**优先读取 MM bearer 的 IP/DNS 属性仍是待实现/验证的改进点**；还需确认该 MM 版本是否有可用 PCO 接口，不能假设 D-Bus IP 配置天然带 P-CSCF。

### 4.6 P-CSCF 候选和 DNS【S07】

当 bearer 配置没有 P-CSCF，当前会做 **6 轮 / 1 秒间隔 / 总预算 12 秒**的活动 AT 上下文只读补查；按同 modem、同 APN 筛选 active context 集合，固定非空集合并前后复核，拒绝矛盾/重复观察。不重写 profile，不另行激活 PDP。

**关联强度的限制**：此补查函数没有接收私有 MM bearer 的实际 PDP CID；它可遍历多个同 APN active context，优先配置 CID 再按编号排序。因此它证明了观察集合稳定，**尚不等于证明候选必定属于本次私有 bearer**。后续应补 owner/bearer↔CID 对应证据，不能把“只读且同APN”误称为完整所有权校验。

每个本地地址族的实际发现优先级：

```text
同族环境显式IP → bearer/活动context P-CSCF → effective配置数值IP
  → IMS同族DNS：配置hostname → pcscf.<home>
  → _sip._udp.pcscf.<home> / _sip._udp.<home> 的SRV目标
```

关键限制：

- 数值 P-CSCF 默认端口 5060；SRV 保留返回端口，不将 TCP SRV 目标作为 UDP 使用。
- DNS 查询绑定 UE 的 IMS local/interface，不调用宿主默认 resolver。
- 校验事务 ID、问题名/type/class/flags，只接受 Answer 中目标或有界 CNAME 链，不把 Additional/Authority glue 当 SIP 地址。
- 数值网络候选可以逐个尝试；**DNS 当前仅返回首个可解析 endpoint**，尚非完整多目标故障回退、SRV weight 调度或 RFC3263/NAPTR 实现。
- `pcscf.<home>` 是代码的发现候选，不是网络保证存在的域。IMS DNS 缺失时，这些规则不会凭空提供一个 DNS/P-CSCF。
- 当前 bearer 候选优先于 profile 数值候选，所以“override”一词不应被理解为所有情况下无条件压过网络配置。

### 4.7 REGISTER、AKA 与安全处理【S08、S09】

- 进入 P-CSCF 前确认 family、source address、provider/worker 仍有效；在 UE 建 UDP socket，按策略准备初始 Authorization、安全 offer 与动态接入身份。
- 共享 REGISTER driver：按 Call-ID/CSeq/方法关联响应；401/407 最多两轮认证（含 AUTS）；423 最多两轮，Min-Expires 上限 **86400 秒**；UDP T1=500ms、T2=4s、TimerF=32s；接受最终 2xx。
- AKA 必须调用真实 USIM，取得 RES/CK/IK 或 AUTS。代码没有由 IMSI“派生 Ki”的机制；初始空 Authorization、Digest 运算均不能替代 SIM 鉴权。
- Security-Server 符合所发 offer/策略才建 XFRM；required 缺失则失败，auto 未协商可走允许的 UDP。受保护刷新不能因 timeout 自动转明文。
- 本地 `port_uc` 是发送端，`port_us` 是接收/Contact/Via 广告端；不能混用而造成“注册成功但呼入无人接收”。
- 最终 2xx 还需验证 Contact/outbound 等 artifacts、提交安全状态、保存 Service-Route/关联身份和租期，再进入 Registered/监听。成功并不等于 IPsec、双注册、短信或语音都通过。

### 4.8 重试有不同层级，不能说成一个总开关【S09】

| 层级 | 当前边界 |
| --- | --- |
| 单次 REGISTER transaction | 认证、423、重传都有次数/时间上限 |
| 同 P-CSCF 的 REGISTER 形状候选 | 每 P-CSCF 有界，最多 24 轮；主要在受允许的 pre-auth 拒绝/sec-agree/漫游条件下变化；不复活显式 disabled 能力 |
| P-CSCF / family 转移 | 只把特定发现/初始传输失败作为可换条件，不将真实 AKA/SIP 拒绝普遍当成换族依据 |
| 外层三槽 profile | generation/线路不可用/基带 wedged 等停止；**并没有统一的“认证后失败一律禁止后续 profile”门禁**，不能用局部阶梯的限制概括它 |
| 资源故障 | 失效 owner、worker generation、基带 unsafe hint 有独立终止/恢复语义，不能无限重拨掩盖 |

derived 的访问网头是受限兼容候选，不改变 home profile/domain。不要把某个窄条件 403 fallback 描述成所有 403 都允许换头重试。

### 4.9 按原会话和真实租期自然续期【S10】

- 响应 Contact 的 expires 优先于响应 Expires，再回落 profile 默认。
- 租期 **>1200 秒时提前 600 秒**；短租期在半程刷新，例如 7200→6600 秒。
- 保留原 bearer、worker、channel、REGISTER identity、Call-ID；CSeq 递增，基于已保存 challenge/AKA 材料重新算 Digest/nc，处理 nextnonce。
- 受保护刷新走旧 SA；有新挑战时暂存新 SA，成功后 commit，失败 rollback。直达 2xx 不擅自更换有效 SA。
- 可重试故障且原租期仍有效时可保持已注册，在不超过剩余租期的 30 秒内原通道再试；过期、flow/owner 丢失或终局拒绝才重建接入。
- refresh 成功增加 refresh 计数，不增加 reconnect、不重置初始 session 时间。重新 initial register 不能冒充原会话自然续期。

这是当前实现应保留的优势，不应退回固定 2700 秒重入整套初始 helper。

## 5. O → P：哪些是新补强，哪些早已存在

| 方面 | 对照结果 |
| --- | --- |
| MM primary bearer 生命周期 | `ims_bearer.rs`、`primary_ims_session.rs`、`primary_ims_lifecycle.rs` 在两基线间无差异；MM unique-owner、AT settings 来源和首族限制均旧已有 |
| 标准 derived / REGISTER core | `profiles.rs`、共享 `core/register.rs` 无差异；LTE 空 AKA 和真实租期续期不是本轮才发明 |
| EF_AD | P 新增严格 CRSM + 前后 CIMI；文本 parser 也取低半字节。O 的 **UIM 原始字节 parser 已取低半字节**，不能说旧版全链不支持 0x82/0x83 |
| UICC/AID | P 先发现当前 slot 完整 AID，再做身份 fallback；更严格处理字段、续行与非法长度 |
| home 消歧 | P 增加 custom/catalog 边界一致性判断，不按最长前缀猜 |
| PDP 写保护 | P 只定义缺失且 inactive 的 preferred CID，拒绝占用/歧义；O 的缺匹配路径保护较弱 |
| 活动 P-CSCF / DNS | P 增加有界轮询与一致性复核、严格 DNS Answer/CNAME 校验、SRV 端口保留及 UDP/TCP 区分 |
| UE/generation/unsafe | P 强化跨 await 的 worker binding、provider unsafe hint 传播，避免同名替换实例误用或误清理 |

历史 SIM-03 在 O 的 `684e2a7` 已完成 derived 首槽 IPv4/UDP 注册和一次原通道自然续期；不能把这归功于 P 的 native 开发，也不能扩张为所有卡、库专属参数或 IPsec 通过。

## 6. 学习 beta8 的优点，但不要移植它的风险

| 主题 | beta8 可借鉴点 / 已见行为 | 当前应采取的方向 |
| --- | --- | --- |
| 身份 | MM/SIM、EF_AD 多来源 | 保留 P 的严格长度、slot、IMSI 稳定性及歧义拒绝；不引入三位猜测 |
| 初始 AKA | 身份在初始 REGISTER 中明确声明 | 当前 LTE 已实现；补回归，不重复“修复”已存在代码 |
| PDP 与 family | 租用、按实际尝试准备定义 | 在 MM 内实现可恢复、能力受限的精确 family 语义；不直接重放 beta8 AT/WDS 流程 |
| P-CSCF | 活动上下文有限等待；自有 WDS IPv4查询 | 轮询已移植；优先补 MM IP/DNS读取和可观测性，不能借 MM client 做QMI捷径 |
| 普通数据/IMS配置 | beta8 有普通数据APN前置分支 | 当前 IMS-only 应保持职责独立，不能为参考版注册测试擅自启普通数据 |
| 安全算法/校验 | MD5-96/null，所读路径未见完整offer匹配检查 | 仅在有网络 offer 和明确策略时评估兼容机制；不全局降级或删除当前验收逻辑 |
| 回退/清理 | IPsec runtime Err后尝试plain，并调用XFRM flush | 保留当前错误分类、受保护flow与精确归属清理，不复制宽泛降级/flush |
| 接入与业务头 | SMSIP、sec-agree、PANI等有真实builder | 对照报文字段与阶段，不照抄占位小区、已禁用头或SMS-only能力声明 |
| 续期 | beta8 有长期运行/定时重调逻辑 | 保留当前网络租期、原flow、nonce、SA事务与失败保留策略 |
| 选源与恢复 | beta8三槽同构机制未确认 | 当前三槽及来源内部fallback单独验收，不以“用了beta8思路”替代证据 |

### 建议后续工作顺序（本轮未实施）

**P0：先解决注册前的信息与语义差异**

1. 在现有 `MmBus`/私有 bearer 中增加 typed `Ip4Config/Ip6Config` 快照，保留 unique owner、对象归属、family/source 校验；明确 MM 版本能力及属性缺失的 fallback，而不是新增硬件 owner。
2. 为 IP/DNS/P-CSCF 逐字段记录来源、对应 bearer/CID、观察时刻与 generation；补齐活动 AT 候选 CID 与本次私有 bearer 的对应关系。MM IP/DNS 不等于 P-CSCF，PCO 能否读取需按实际接口验证。
3. 把线路 `ipv4v6` 的意图与 QCA410/MM 首族行为显式化；离线 fake-transport/CI 覆盖默认顺序、forced-family、重复尝试和实际授予族。若改 PDP lease，必须可恢复且不修改其他业务 context。

**P1：到达 SIP 后再比较认证与兼容性**

4. 同卡、同 MM 模式、同有效 profile 对比初始 Authorization、身份/realm、PANI、Contact 能力、Security-Client/Server；只有真实 challenge/拒绝证据才调整相应规则。
5. 用模拟 registrar 验证 401/407、AUTS、423、终局403、invalid Security-Server、timeout与SA rollback；另评估外层 profile 在认证拒绝后的重试策略。
6. 扩展 DNS 多 endpoint 故障回退与 SRV 调度时，保留现有回答归属、端口和 UE 隔离保护；明确哪些 transport 尚不支持。

**P2：授权窗口内的硬件验收**

7. 由唯一操作负责人核对程序 hash/卡标签/无通话及回滚条件，分别记录初始注册、原会话自然续期、A/B/C 来源回退。不要为了省时间缩租冒充自然续期。
8. 短信、呼叫、视频、Trunk、普通数据和漫游费用策略另行授权、另行验收；本分析不构成操作许可。

## 7. SIM-04 历史失败应如何解释

| 证据 | 可说的结论 | 不能说的结论 |
| --- | --- | --- |
| 9/15 T02 `6391732` 的详细记录 | MM/UE 中建立IPv6 IMS bearer；AT可见P-CSCF/DNS为空；未到SIP/AKA | 网络一定没下发PCO；IMS身份一定错；新修补已通过 |
| JSONL 后续对 `269be65` 的失败摘要 | 会话继续报告三槽停在同一P-CSCF阶段 | 本片段未含其完整原始测试输出，不能当成本轮重新硬件验收或混用T02资产 |
| beta8 旧试运行 `volte_data_path_apn_missing` | 停在不等价的data-path/APN前置条件，不能作为正常beta8注册能力对照 | 两个版本按正确原生配置均无法注册；一定是IMS APN派生失败 |
| home45507 / serving46011 | 历史处于漫游；IMS漫游策略是可能因素 | 仅凭漫游或空AT字段断言运营商未开通 |
| 两实现身份/安全默认不同 | 后续到SIP层时值得对照 | 这些尚未参与的参数已被证明导致当前P-CSCF之前的失败 |

应先区分：**承载授权 → 配置可见性 → P-CSCF发现/可达 → SIP挑战 → AKA → 注册接受**。当前证据不足以在“本地读取/承载准备差异”和“漫游网络策略”之间作最终归因。

旧文档的“该轮没有部署新候选/beta8”等语句有其时间范围；JSONL 含后来的试运行及恢复，不能互相覆盖成一条无时间边界的结论。会话最后恢复记录也不是设备现在仍处于同样状态的保证。

## 8. 尚未确认的 beta8 细节

以下限制保留在文档中，避免未来引用时把静态重建误当完整源代码或安全审计：

- 各驻网模式下 MM home-operator 的完整来源、所有覆盖字段与 ISIM身份读取情况。
- 全部硬件布局、普通数据APN getter字段、MM/自有WDS路径选择及双栈默认/退让顺序。
- 全部 P-CSCF 来源的严格优先级、CID/APN/SIM绑定、DNS解析与候选调度。
- Security-Server 重复参数/多offer等边缘语义、所有调用分支的清理结果、flush执行namespace和密钥生命周期。已确认的宽泛plain回退不再当作未知项。
- 固定刷新调度与最终租期/423的全模式关系、旧SA保留、supervisor完整退避策略。

本文已覆盖端到端功能链；这些是明确的逆向证据缺口，不能通过从当前项目复制逻辑来“补全”。

## 8.1 2026-09-19 深度 IDA 补证：beta8 的高成功率来自分层兜底，不是单一派生字符串

本节是对同一 beta8 样本的第二轮只读核对，目的是把“用户反馈 beta8 对 SIM-04 注册成功”拆成可验证的机制，而不是把整个参考实现照搬到 MM 主线。

### 样本与方法

- IDA MCP 已连接，当前 IDB 为 `simadmin`，base=`0x0`。
- 当前 IDB SHA256=`210c35b11f54dd240a83e90dd08d5e8a8f4f2cea227ce3a0503a9ced4140f9b7`，MD5=`d0903ceab475bacaf00e7ef45d1403c5`，与第 2 节样本一致。
- 本轮只读 metadata、字符串交叉引用、函数反编译/反汇编和当前源码；没有执行 beta8、修改 IDB、连接设备或发送 AT/QMI 命令。
- Rust 反编译中的自动变量和部分函数类型不可靠；下文只把能由调用、格式块、错误字符串和控制流共同支持的行为列为“确认”。

### 端到端流程的真实分层

```text
可靠 IMSI / home PLMN / EF_AD 来源
  -> IMPI、IMPU、realm、registrar 派生
  -> PDP/CID 占用扫描与 profile lease
  -> 按本轮 family 准备 IMS profile / P-CSCF reporting
  -> 选择 MM bearer 或自有 WDS 路径
  -> 当前 context / bearer settings / P-CSCF 多来源读取
  -> family、地址、路由就绪
  -> IPsec-3GPP REGISTER（若路径启用）
  -> plain UDP REGISTER（参考版的宽泛 fallback）
  -> 423 / 401 / 407 / AUTS / UIM AKA
  -> 接受头、租期、监听与后续 refresh
```

因此 beta8 的“高成功率”更合理地解释为：它同时覆盖了多个运营商差异点和多个承载/发现路径；不是某个通用 `ims.mnc...` 域名模板本身能让网络授权 IMS。

### A. 身份派生：必要但不是本轮 P-CSCF 根因

`sub_19014C`（现有 B01/B02）仍能确认以下顺序：

1. 优先通过 MM 代发的 `AT+CIMI` 获取 IMSI；失败时从 SIM/UIM 身份路径补充。
2. home operator、EF_AD 和兼容分支参与 MNC 长度判断；最后仍存在面向特定国家的兼容猜测。
3. 以 home PLMN 构造 `ims.mnc<MNC 三位补零>.mcc<MCC>.3gppnetwork.org`，再形成 IMPI/IMPU/realm/registrar 的 REGISTER 输入。

这解释了 beta8 对多运营商的覆盖面，但 SIM-04 当前失败发生在 P-CSCF 阶段，尚未发送 REGISTER，所以不能把 AKA、realm、Contact 或 IPsec 算法当作已证实根因。当前项目严格的 home/serving 分离、EF_AD 低半字节、前后 IMSI 一致性和 slot-bound AID 仍应保留。

### B. PDP/CID lease：beta8 的关键差异是“按实际 context 管理”，不是随便找一个 CID

`sub_19A008` 的控制流会读取 PDP 定义和活动状态，维护占用集合，并为本轮尝试选择/租用一个 context；已有本程序租用的 context 可以复用。这个事实支持三点：

- APN、PDP type、CID/profile-id 和本轮 family 是一组关联事实，不能只把 `ip-type` 字符串传给 MM。
- 失败、重试和释放都需要携带同一个 lease；不能让后续 forced-family 重试继续使用已经不匹配的 profile pin。
- “profile-id 优先于 ip-type”是 MM 1.18 的实际约束；beta8 的 MM 格式块也同时传了 `profile-id`、`apn=ims`、`ip-type` 和 `allow-roaming`（`sub_196634` / 格式块 `0x518660`）。

当前项目已在 `e8bff12`/`5094ac1` 补上实际 family 投影、pin 保留、v1/v2 receipt 和 retained bearer 校验；仍未完成的是**可恢复的精确 family lease**，而不是再改一个请求标签。

### C. profile 准备：IDA 只确认了命令形状和时序边界

`sub_1A0A80` 的直接格式块/调用点确认：

```text
AT+CGACT=0,<cid>
AT+CGDCONT=<cid>,"<本轮 PDP type>","ims"
AT$QCPDPIMSCFGE=<cid>,1,1,1
```

这说明 beta8 会按本轮 PDP type 准备 IMS profile，并显式开启 P-CSCF reporting；它不是只依赖默认 modem profile。

但必须保留以下证据边界：

- 当前 IDA 片段确认的是 profile 准备命令，不足以证明所有 MM 路径都会先执行一个独立的临时 `CGACT=1` 预取，再把同一 CID 交给 MM/WDS。
- 当前仓库中旧 `prefetch_pcscf_from_ims_profile` helper 确实包含 `CGACT=1 -> CGCONTRDP -> CGACT=0`，但它没有被 5094ac1 的生产 live 路径调用；本轮审阅确认不能仅凭函数注释把它重新标成 beta8 已证实行为。
- QCA410 历史材料记录过临时 AT 激活与后续 WDS 接管之间的 firmware/DHCP context 释放竞态；因此不把该 helper 直接接入默认 MM 路径。它还存在完整 `CGDCONT` 字段、原始 reporting 值、删除/恢复和取消清理不足的问题。
- 已有活动 IMS/EPS context（SIM-04 当前观测为 CID 2 active）时，任何 `CGACT=0`、`CGDCONT` 重写或 reporting 改写都必须视为维护操作，不能由普通注册重试自动执行。

可安全借鉴的是“把 profile type/pin/reporting 当作同一 lease 的字段”这一设计原则；不可直接借鉴的是没有完整 owner/事务锁/精确恢复的临时激活脚本。

### D. 两条承载路径必须分开

`sub_196634` 是 MM bearer 路径：格式中同时出现 `profile-id`、`apn=ims`、`ip-type`、`allow-roaming`，并沿 MM 对象读取/连接/检查结果。

`sub_19D6DC` 是另一条自有 WDS 路径：可分配并保持自己的 WDS client，设置 family，以 `apn=ims,3gpp-profile=<cid>,ip-type=<family>` 启动，再通过 WDS current settings 查询地址/PCO；已见的 `0x002d` / mask `0x0c00` / TLV `0x23` 是 IPv4 P-CSCF 列表路径，IPv6 分支不走这条查询。

这解释了 beta8 为何能覆盖更多设备/网络组合，但也决定了当前项目不能这样“修复”：

- 不能从 MM bearer 内部借一个 WDS client；
- 不能让 MM 和应用同时成为同一 modem 的 bearer owner；
- 不能把 DATA6 或宿主 namespace 当作注册后门；
- 不能把 beta8 的自有 WDS IPv4 P-CSCF 查询当成 SIM-04 IPv6 解决方案。

当前项目的正确移植目标是：MM provider 只通过自己的 retained bearer 提供 IP/family/owner 快照，应用只在同一 retained session 上做只读 P-CSCF 关联。`5094ac1` 已实现这一边界。

### E. P-CSCF 发现与等待

`sub_19B9F4` 的静态控制流确认：

- 没有预取结果时，活动 context 读取是有界的，最多 6 轮，轮间约 1 秒；
- 每轮读取当前活动 context/定义和 `CGCONTRDP`，再继续后续注册路径；
- 发现结果之后才进入路由、IPsec/plain runtime 和 REGISTER。

这部分是当前项目可以安全复用的 beta8 优点。当前代码已经有 6 轮/12 秒的只读等待、严格活动行/定义行解析、DNS Answer/CNAME/SRV 归属校验；`5094ac1` 又补了 MM retained bearer 的 owner/profile/IP 前后复核，以及 IPv6 同 `/64` 不同 IID 的窄关联规则。

但“多读几次”不等于网络一定会下发 PCO。9/19 `5094ac1` 候选实测仍记录：MM 获得 IPv6、DNS/PCO 为空，三槽停在 P-CSCF；当前 `CGCONTRDP=2` 仍只有 7 个字段。没有同条件 beta8/current A-B 报文，不能继续推断唯一根因。

### F. REGISTER/AKA 和安全回退

`sub_18D548`/`sub_197CC8` 与现有 B06-B08 一致地支持：

- 初始空 AKA Authorization；
- 423/Min-Expires 有界调整；
- 401/407 challenge、真实 UIM AKA、AUTS 分支；
- IPsec-3GPP Security-Client/Server/Verify 和后续受保护 REGISTER；
- plain UDP runtime 与固定/重入式 refresh 路径。

beta8 的高兼容性行为中有两项不能直接移植到当前 LTE/MM 主线：

1. IPsec runtime 出错后不按错误类型筛选就清理并转 plain UDP；
2. 清理 helper 使用宽泛 `ip xfrm policy flush` / `ip xfrm state flush`，且当前证据不足以证明只影响本会话 namespace。

当前项目的更严格 Security-Server/SA/owner/generation 保护应保留。SIM-04 尚未到 SIP/AKA，所以现在改 REGISTER、AKA、MD5/null 或 Contact 只会扩大变量，不能解释当前失败。

### G. 续期行为的差异

beta8 可见 2700 秒调度，并可能重新调用初始 helper；当前项目按网络协商租期、原 bearer/worker/channel/Call-ID/CSeq/AKA/SA 维护自然 refresh。后者更适合作为项目基线，不能用 beta8 的固定调度替换。

## 8.2 当前项目采取的修复边界

本轮没有把旧临时 AT 激活 helper 接入生产路径。理由是：IDA 对 `sub_1A0A80` 的证据足以支持 profile 准备的命令形状，但不足以支持该 helper 的完整临时激活/恢复语义；而当前 QCA410 既有失败记录显示第二个激活 owner 存在风险。

当前代码保持以下安全路径：

1. 由 derived/catalog/database 解析出 APN、home domain、IMPI/IMPU、initial AKA 和 family 意图；
2. MM 以 profile pin、APN、family、漫游策略创建自己的 bearer；
3. 从该 bearer 的 typed IP4/IP6 snapshot 读取实际授予 family、地址、网关、DNS；
4. 只在 provider retained session 内读取/关联 P-CSCF；
5. 以同 family、同 owner、同 worker generation 的地址进入 UE namespace，再进入 SIP/AKA；
6. P-CSCF 缺失时保留分层失败，不把 DNS/IP/配置猜测伪装成注册成功。

`5094ac1` 还收紧了 pinned profile 的 forced-family 重试：MM 1.18 在有效 `profile-id` 存在时以 profile 内 PDP type 为准，继续只改请求标签会重复同一 PDN 失败；当前返回明确的 profile-pin/family conflict，等待独立 exact-family lease，而不是丢 pin 或覆写现有 profile。

当前尚未完成、但需要单独设计的下一项是 **MM 内的 exact-family profile lease**：需要完整记录 profile 定义全部字段、原始 reporting 状态、CID 存在性、MM owner/bearer、创建/取消/崩溃恢复和最终验证；不能通过普通 retry 自动执行 `CGACT`/`CGDCONT` 重写。

## 8.3 对当前失败的准确解释

- 5094ac1 的代码、CI、35 项新增 Rust 回归、arm64/amd64 候选包均已验证；这不是实机注册通过。
- 9/19 独立 MM 候选窗口没有重新附着，实际三槽仍停在 P-CSCF：首/末槽 `context_pcscf_absent`，中间槽 `at_response_invalid`；没有 SIP/AKA。
- 9/18 重新附着后出现 P-CSCF 候选，说明上报时序/初始 EPS 状态可能是变量；它不证明 profile 派生、beta8 预取或运营商授权中的任何单项已经确定为根因。
- 下一次若允许维护窗口，应只做一个变量：经 MM 受控重新附着，保持 APN、Initial EPS、MM owner、Wi-Fi 和 profile 顺序；先核对上报字段和 retained bearer，再决定是否进入注册。不能把该操作自动化为普通连接 retry。

## 8.4 已确认与未确认清单

**已确认：**

- beta8 样本身份与同哈希 IDA；
- 动态 IMS 身份/home domain 派生；
- PDP/CID lease 及 profile 准备命令形状；
- MM 与自有 WDS 两条承载路径；
- IPv4 WDS P-CSCF 查询的消息/mask/TLV；
- 活动 P-CSCF 有界读取；
- REGISTER/AKA/IPsec/plain/refresh 的主要分支；
- 当前项目已实现 MM typed IP/family、retained owner、P-CSCF 关联和严格 DNS 边界。

**未确认：**

- SIM-04 beta8 成功路径究竟走 MM 还是 direct WDS；
- beta8 是否在该路径实际执行临时 `CGACT=1` 预取；
- beta8 完整 profile 全字段、reporting 原值和 cleanup 语义；
- AT CID、MM profile-id、Initial EPS profile 是否一一对应；
- 临时 AT context 的 P-CSCF 是否可用于后续独立 MM bearer；
- exact-family lease 是否实际提高 SIM-04 成功率；
- 同卡同网络的 beta8/current SIP/AKA A-B 报文。

## 9. beta8 二进制证据索引

以下均针对第 2 节同一 SHA256，采用 IDA base=0；函数名是分析数据库自动名。

| 编号 | 函数 / 指令位置 | 证实点 |
| --- | --- | --- |
| B01 | `0x913F8` 内 `0x914B4 → 0x19014C`；后者 `0x19098C–0x190C38`、`0x1915BC–0x191684`、`0x19186C → 0x1A8674` | CIMI/fallback、home长度选择、域构造、进入承载主线 |
| B02 | `0x192288`（长度/前缀helper）、`0x191600–0x191634`（460/三位fallback）、`0x1912A4–0x191460`（CRSM）、`0x411890`（payload[6..8]） | 可靠来源优先但有猜测；AT EF_AD完整字节02/03限制 |
| B03 | `0x1A86BC–0x1A87AC`、`0x1A9560–0x1A9704`；getter `0x1AC8D8` | 普通数据APN候选、内置APN、MM bearer枚举及缺APN错误 |
| B04 | `0x19A008`（lease）；`0x1A0A80` 内 `0x1A0AB0/0x1A0AE8/0x1A0B44`；`0x196658`（env路径）；`0x19D6DC`；`0x1AA2F8 → 0x3F0BA8 → 0x19B9F4` | PDP按family准备、reporting、MM对象路径、自有WDS、承载到注册的尾调用 |
| B05 | `0x19B9F4` 内 `0x19C00C/0x19C02C/0x19C22C`；`0x18F390`；`0x19E218/0x19E3DC/0x19E3F4/0x19E518` | 有界活动读取、地址parser、WDS0x2d/mask0x0c00/TLV0x23 IPv4列表 |
| B06 | `0x18D548`，重点 `0x18D724/0x18D88C/0x18D89C/0x18D964/0x18DBA8/0x18DCC4/0x18DF04`；`0x1AC338`、`0x18CED8`、`0x1A43B4` | 空AKA、CSeq/租期、423/401/407/200条件与REGISTER字节构造 |
| B07 | `0x18A810`；`0x1BDDCC` 完整主体；`0x1BD6C8`、`0x1BD500`、`0x1BD3C0`；`0x195F20/0x18FB04` | AID、@qmi-proxy UIM、APDU、90/00及DB/DC解析、清理调用、Digest |
| B08 | `0x197CC8` 内 `0x198000/0x1981C0/0x1984D8/0x1985E8/0x1986A8/0x198EF0/0x1990F8/0x199650/0x199258`；`0x18D1E4`、`0x189828`；`0x19CD50–0x19D018`、`0x18A490` | offer、423/挑战、单次AUTS、精确200、Security-Server提取、runtime Err转plain与flush |
| B09 | `0x1A4DD4` 内 `0x1A5074/0x1A6468/0x1A6588`；`0x1A0D14` 内 `0x1A0D84/0x1A2138/0x1A229C`；`0x3EB03C → 0x35C1B4` | 初次/定时重调注册helper，2700秒时间加法 |

WDS 常量对照资料：[libqmi 1.28.6 WDS消息定义](https://github.com/linux-mobile-broadband/libqmi/blob/1.28.6/data/qmi-service-wds.json)、[WDS枚举](https://github.com/linux-mobile-broadband/libqmi/blob/1.28.6/src/libqmi-glib/qmi-enums-wds.h)。标准域及SIM元数据涉及 TS23.003/TS31.102，REGISTER与IMS安全涉及 TS24.229；标准说明不代替二进制实际实现证据。

## 10. 当前项目源码证据索引

根目录默认为 **P/`backend/src/`**。为缩短引用：

- `CI/` = `connectivity/modems/ims/cellular_ims/`
- `VF/` = `connectivity/modems/ims/vowifi/`
- `IMS/` = `connectivity/modems/ims/`
- `Q/` = `hardware/devices/qcm410/`
- `CORE/` = `connectivity/core/`

行号固定于 `269be65` 的本次工作树；后续代码移动时以函数名和 commit 定位。

| 编号 | 文件与行号 | 入口/用途 |
| --- | --- | --- |
| S01 | `hardware/cellular/backends/config.rs:7–31,109–120`；`hardware/cellular/control.rs:564–583`；`CI/live.rs:6853–6870` | BackendMode默认MM、AT代发、UICC发现 |
| S02 | `CI/live.rs:6628–6912`；`CI/identity.rs:43–158,164–283,335–374`；`VF/qmi_uim.rs:506–511,915–1013` | load_device_identity、严格EF_AD、slot AID、USIM身份 |
| S03 | `VF/profile_store.rs:740–1005,1189–1220`；`VF/carrier_catalog_v7.rs:484–610`；`platform/config.rs:4203–4271,4874–4909` | resolve_cellular_ims_candidate、automatic_home_plmn_hint、来源/ready及三槽 |
| S04 | `VF/profiles.rs:979–1001,1088–1254`；`IMS/effective_profile.rs:189–240`；`CI/live.rs:6830–6850` | 标准derived默认、override、IMPI/IMPU实际构造 |
| S05 | `CI/plan.rs:303–351`；`CI/live.rs:2053–2165`；`CI/pcscf.rs:236–340`；`CI/native_bearer.rs:262–382` | 线路family计划、PDP准备、reporting、transport请求 |
| S06 | `Q/ims_bearer.rs:147–349`；`Q/primary_ims_session.rs:54–143,295–370,460–551`；`Q/primary_ims_lifecycle.rs:148–343,420–562`；`hardware/cellular/cgcontrdp.rs:50–194`；`CI/native_bearer.rs:112–197` | 首族/MM owner/lease、实际settings来源、UE迁移 |
| S07 | `CI/live.rs:2235–2367,2628–2680`；`CI/pcscf.rs:473–590,747–960`；`CI/pcscf_dns.rs:187–334`；`CI/bearer.rs:625–728` | active P-CSCF、DNS/端口、worker路由/源地址 |
| S08 | `CI/live.rs:817–839,1345–1698`；`CI/identity.rs:352–374`；`VF/qmi_uim.rs:1085–1181,1258–1270,1393–1417`；`CI/digest_aka.rs:64–111` | 初始Authorization、真实AKA、proxy调用与transport重试 |
| S09 | `CORE/register.rs:16–103,220–400,524–635`；`CI/live.rs:2681–3088,7338–7599`；`CI/ipsec.rs:344–449,524–563`；`CI/channel.rs:514–540`；`api/handlers.rs:10431–10461,10804–10955` | REGISTER、安全端口/协商、成功状态、局部与外层不同失败门禁 |
| S10 | `CORE/registration.rs:36–59`；`CI/live.rs:495–515,662–751,3465–3551,3712–4071`；`CI/runtime.rs:634–724` | 租期、原flow刷新/nonce/SA回滚、profile结果记录 |

## 11. 相关文档与可复核材料

- [原 IMS 派生专项交接](../../../.local/archive/legacy-docs/IMS_DERIVED_FALLBACK_HANDOFF.md)：本地操作约束、历史 SIM-03 验收、单写入者与费用边界；不应强制加入公开版本管理。
- [此前 SIM-04 P-CSCF / beta8 对照](IMS_PCSCF_BETA8_COMPARISON_2026-09-15.md)：详细 T02、WDS指令及后续修补记录。
- [当前主要项目逐卡交接](PROJECT_HANDOFF_2026-09-12.md)：历史测试按卡/版本/轮次记录，不用新结论覆盖旧失败。
- [REGISTER 三态字段](../../IMS_REGISTER_TRISTATE_SCHEMA.md)：显式disabled与缺省不同，兼容fallback不能复活明确禁止项。

相邻工作树链接用于本机阅读；其他环境应按第2节分支/commit获取对应文件，不假设该目录布局存在。

本地辅助材料在 `.tmp/session-resume/`：全会话提取、beta8证据底稿、项目源码底稿、UIM/EF_AD补证；beta8静态缓存在 `.tmp/reference-beta8-20260915/`。这些不随普通clone提供。正式结论的关键地址和源码位置已收进本文，不依赖凭据、完整IMSI、AKA材料或设备原始日志。

**交付性质**：本文是文档与静态分析成果。本轮未改业务代码，未运行Rust构建/测试，未进行硬件注册验收；后续实现与设备测试应依第6节单独执行和记录。
