# 架构与网络隔离

本页只解释当前设计，不把历史部署、候选验证或开发计划写成实时状态。
当前工作与设备最后证据见 [HANDOFF](HANDOFF.md)，协议细节见 [IMS 注册与兜底](IMS_REGISTRATION_POLICY.md)。

## 1. 依赖方向

- `api`：认证、HTTP请求/响应、线路解析，不直接实现设备协议。
- `connectivity/core`：接入无关的SIP、Digest-AKA、注册、短信和语音模型。
- `connectivity/modems/ims/{cellular_ims,vowifi}`：各自的接入、安全通道和注册适配。
- `hardware`：发现、SIM与设备生命周期；具体QCM410/Quectel/DJI能力在驱动边界内。
- `services`：线路registry、UE worker、跨接入策略、自动化/通知/Trunk。
- `platform`：配置、数据库、DNS、命名空间、路由和系统工具。

同一物理设备必须只有一个协议owner；MM与native不能同时写同一端口。
详见 [设备驱动](DEVICE_DRIVERS.md)、[原生后端](NATIVE_BACKEND_STATUS.md)。

## 2. 线路身份与SIM身份分离

管理单位是物理线路：基带卡槽加UIM slot，或独立读卡器。
`physical_line_id`依据物理硬件锚点和slot生成，不包含ICCID；slot1保留兼容形式。
因此同槽换卡不丢失用户的线路开关、代理、Trunk和候选顺序。

SIM绑定独立使用ICCID，eSIM可使用EID+profile ICCID；按SIM的覆盖不能套到另一张卡。
旧包含SIM的line ID通过`legacy_line_ids`迁移配置引用；短信/通话历史保留写入时的ID，不批量改写事实。
设备对象路径、数字CID、MM profile-id、eSIM profile与逻辑通道编号不相互等价。

换卡或MM对象变化先使旧任务失效，再以当前owner/SIM/代次验证资源；
未知状态不是“没有资源”，不能复用同号对象替代原归属。
详见 [MM租约与恢复](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)。

## 3. 原线路工作台

SIM页面保留左侧线路列表、右侧选中线路的概览、eSIM、IMS与Trunk、短信、补充业务、自动化、通知标签。
读卡器是独立线路，基带专属能力按设备类型限制。

- 物理设备在而SIM缺失时仍应显示线路，不因IMS不可用清空整个页面。
- 启动恢复阻塞时，只读MM缓存可以生成**序列化展示投影**，包括真实保存配置及`read_only`/`blocked_reason`。
- 展示投影不加入操作registry，不创建LineRuntime、worker、namespace或SIM通道。
- 原卡片、选择和标签仍可阅读；危险操作/自动探测不执行，未知状态明确标示。
- 禁止另建替代清单把原工作台隐藏，也不能把保存的开启意图伪造为关闭。
- 正常离线已登记线路的配置保存，与启动门禁下仅供展示的投影，是两种不同状态。

摘要接入优先显示已注册VoWiFi、已注册蜂窝IMS、正在连接的已启用接入，最后才是CS。
modem连接、应用运行或进度条完成不能单独证明IMS/语音/音频可用。

## 4. 强制per-UE网络命名空间

每条活动线路拥有一个Linux network namespace和UE worker；不是可关闭的部署模式。
旧`ue_isolation`开关已移除，未知顶层字段应拒绝；配置版本4文件不能凭空视为已迁移到版本5。

固定布局由代码维护：namespace前缀`sa-ue`，host veth前缀`savh`，UE veth前缀`save`，veth MTU1500。

- VoWiFi：ePDG/IKE/ESP/TUN/SIP/XFRM及运营商媒体socket在对应线路namespace。
- 蜂窝IMS：承载网卡、SIP/XFRM及运营商媒体socket在对应线路namespace。
- 普通数据代理和自有DATA承载使用对应UE出口。
- host veth提供namespace出站/NAT，不是失败后偷用管理网的IMS数据面。

namespace、worker、veth、接口迁移或socket创建失败时，相关路径不可用；不得在host namespace偷偷重试。
回收时将自有接口移回host再释放bearer属于生命周期清理，不是运行时host兜底。

## 5. 路由域与socket绑定

`platform/network_routing.rs`分配路由域：

| 域 | 表基址 | 规则优先级基址 | 用途 |
|---|---:|---:|---|
| ModemData | 12000 | 10000 | 数据代理/流量 |
| VolteIms | 14000 | 14000 | 蜂窝P-CSCF、RTP/RTCP/视频 |
| VowifiIms | 16000 | 18000 | VoWiFi隧道、P-CSCF和媒体 |

具体表号结合稳定接口槽位和地址族；v4/v6不覆盖彼此。动态P-CSCF和媒体路由不得写入共享主表。
每个实际P-CSCF候选进入注册前必须有正确的承载路由，不能只配置第一个地址。

源地址规则仍需配合socket绑定和namespace：同地址可能出现在不同线路，单靠`from`规则不够。
数据代理TCP、IMS信令/媒体/P-CSCF DNS、VoWiFi TUN流量均必须保持自己的接口和运行上下文。
外部进程或透明转发不自动继承这些socket约束；新增入口必须明确namespace/fwmark/VRF等隔离契约。

## 6. DNS

普通系统解析统一使用`platform::dns`的Hickory Rust解析器：

1. 数字IPv4/IPv6（含方括号IPv6）不查询DNS。
2. hosts先于resolv.conf；hosts命中不能因resolver配置不可读失败。
3. 读取系统nameserver/search/options，A/AAAA均请求，整体网络查询预算4秒。
4. 使用`UserProvidedOrder`保持系统服务器顺序，不能被新resolver随机初始RTT重排。
5. 空结果/错误为失败；通用DNS不注入公共服务器。既有ePDG显式公共回退由调用点控制。

每次查询拥有新的有界resolver，不建立跨runtime/namespace的全局socket/cache；线路ePDG缓存另行维护。
HTTP客户端及SOCKS代理端点共享此系统解析入口，TS.43地址pin/重定向校验保留。

运营商DNS、P-CSCF/NAPTR、SOCKS5 UDP DNS有独立Rust传输/路由，不能以重构为由偷换成host DNS。
该设计保持调用者已有上下文，不宣称所有父进程DNS都已经搬入worker。

## 7. Profile与配置

归属PLMN来自SIM/ISIM/USIM EF_AD等可信订阅事实及有歧义保护的catalog推断；驻网PLMN不能替换归属身份。
三个来源槽（用户database、只读carrier_catalog、derived）按线路保存。
候选来源缺失或不可用时可回退derived，并明确记录requested source、effective origin及fallback reason。
旧通用显式pin接口的严格失败，与线路候选接口的来源内回退，不可混为一谈。

主设置为保留注释的YAML/JSON文本；线路、SIM覆盖、自动化、通知和运行历史存SQLite。
配置管理内存模型仍统一，只有持久化层分拆。迁移只能改声明兼容的字段，不能改写旧业务事实或清空数据。
路径、备份和服务说明见 [安装与运行](INSTALL.md)，来源契约见 [运营商配置](CARRIER_PROFILES.md)。
