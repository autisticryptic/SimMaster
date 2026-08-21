# IMS 与数据接口路由隔离

## 背景

SimAdmin 同时管理多种网络接口：

- 普通数据承载，例如 `wwan0`、`wwan1` 或 USB modem 的 MBIM/QMI 接口；
- VoLTE IMS 承载，通常复用 `wwan` 数据面，但必须绑定到对应线路；
- VoWiFi IMS 的每线路 ePDG TUN，例如 `sa_vwf...`；
- 管理网络，例如 `wlan0`、以太网或宿主机默认路由。

运营商在 SIP/SDP 中给出的 P-CSCF、RTP、RTCP 和视频地址通常不是同一个地址。只把 P-CSCF 写入主路由表会导致动态媒体地址落到管理网口；多卡同时运行时，后建立线路的 `/32` 路由还会覆盖先建立线路。

## 路由域

`backend/src/platform/network_routing.rs` 统一分配路由表和规则优先级：

| 域 | 表号范围 | 用途 |
| --- | --- | --- |
| `ModemData` | `12000+` | 普通数据代理和流量连接 |
| `VolteIms` | `14000+` | VoLTE P-CSCF、RTP、RTCP、视频 |
| `VowifiIms` | `16000+` | VoWiFi ePDG TUN、P-CSCF、RTP、视频 |

同一域内由接口名槽位和 IPv4/IPv6 家族生成稳定表号。`wwanN` 使用 `N`，TUN/USB/MBIM 等其他接口使用接口名哈希，避免线路重启后随机变化。

每个承载建立一条源地址规则：

```text
ip rule add priority <line-priority> from <ims-address>/32 table <line-table>
ip route replace <remote-media>/32 dev <line-interface> table <line-table>
```

IPv6 使用 `/128` 和 `ip -6`。这样相同的远端 RTP 地址可以同时出现在多条线路中，Linux 会按源地址选择正确的线路表，而不是按主表的最后一条路由选择。

### 完全相同本地 IP

如果两个接口同时拿到完全相同的地址（例如 `wwan0: 10.0.0.2`、
`wwan1: 10.0.0.2`），`ip rule from 10.0.0.2/32` 本身无法区分接口。因
此，源地址规则只作为路由表和动态目标路由的补充，不能作为这个场景的唯一隔离
手段。所有会主动建立连接的线路 socket 还必须绑定接口：

- 数据代理的每个 TCP 出站 socket 使用 `SO_BINDTODEVICE`；
- VoLTE SIP、P-CSCF DNS、RTP/RTCP/视频 relay socket 绑定对应 `wwan*`；
- VoWiFi SIP/RTP socket 绑定该线路独有的 `sa_vwf...` TUN；
- QMI 数据承载识别探测 socket 绑定当前候选 `wwan*`，避免把另一条线路的
  DNS 响应误判为当前承载。

这样，当前项目内的代理、流量消耗、IMS 信令和 IMS 媒体在完全相同 IP 的场景
下仍按接口隔离。仍然没有显式 socket 的第三方进程或内核透明转发流量，不能靠
`from` 规则可靠区分；这类需求需要为每条线路增加 `fwmark/connmark`、VRF 或
network namespace，并由所有入口统一继承线路标记。

### 数据代理与自动化流量

普通 HTTP/SOCKS5 代理和自动化的“消耗流量”任务都调用同一个线路代理运行时。
代理为每个目标地址创建新的 `TcpSocket`，先设置 `SO_BINDTODEVICE`，再连接远端；
不会复用宿主机连接池或另一条线路的 socket。自动化任务即使该线路的持久化
“数据连接”开关关闭，也只临时启动一个 `127.0.0.1`、随机端口的代理，任务结束
后恢复原来的开关状态。流量任务使用固定的 Cloudflare HTTPS 地址列表，避免这条
任务的 DNS 查询落到管理网络；普通代理的域名解析仍由宿主机 resolver 完成，但
实际 TCP 连接仍强制绑定线路接口。

### VoLTE profile 与漫游

VoLTE 的 profile 选择以 SIM/IMSI 的归属 PLMN 为准，绝不会把
`modem.3gpp.operator-code` 在漫游时直接当成归属运营商。归属 PLMN 的来源按以下
顺序使用：SIM 对象属性、与 IMSI 前缀一致的已注册运营商、USIM EF_AD 的 MNC 长度，
最后才交给 catalog 按 IMSI 推断。自动匹配找不到可用 LTE profile 时，会生成带有
`ims.mncXXX.mccYYY.3gppnetwork.org`、`ims` APN 和通用 SIP REGISTER 策略的标准
3GPP 派生 profile，并在运行状态的 `profile_source=derived` 和
`profile_fallback_reason` 中明确标记；显式 pinned profile 仍严格失败，不会被默默
替换。

当当前注册 PLMN 与归属 PLMN 不同，且 profile 允许访问网络头时，REGISTER 会动态
加入当前驻留网络的 `P-Visited-Network-ID`。这个头只影响漫游上下文，IMS 域名、APN、
AKA、registrar 和安全策略仍来自归属 profile。标准派生 profile 也可以使用这个
动态头，因此没有数据库记录的卡仍有一次通用漫游注册机会。

## VoLTE 生命周期

1. IMS bearer 建立后，为 IPv4/IPv6 地址创建 `VolteIms` 源策略和承载连接路由。
2. P-CSCF 发现时写入对应 `VolteIms` 表。
3. 初始来电 INVITE、183、200 OK 和后续 re-INVITE 的 SDP 都解析音频和视频媒体 IP，并写入同一承载表。
4. RTP socket 继续使用 `SO_BINDTODEVICE` 绑定承载接口；策略路由解决的是动态目标地址选择，二者共同防止媒体逃逸到 `wlan0`。
5. bearer 释放时只删除本线路的表和规则，并清理承载接口地址；不会刷新其他线路的表。

## VoWiFi 生命周期

1. 每条线路的 ePDG TUN 创建后建立 `VowifiIms` 源策略。
2. P-CSCF 和动态 RTP/视频地址进入该 TUN 的独立表。
3. TUN 删除前清理对应表和规则，再删除接口；不会影响其他 VoWiFi 线路。

## 新设备接入约束

设备适配层只需要提供：

- 承载实际接口名；
- 承载 IPv4/IPv6 本地地址和前缀；
- 建立和释放承载的生命周期钩子。

不得在设备或 IMS 实现中直接向主路由表写入动态 P-CSCF/RTP/视频路由，也不得用全局固定接口名（例如 `wwan0`）替代线路绑定。新增 EC20/EC25/EG25/EG600 或 USB modem 时，应复用 `RouteDomain`，并为多线路、相同远端 RTP 地址、重连和释放增加隔离测试。

## 当前验证

- Windows host `cargo check`：通过。
- WSL Debian Linux `cargo check`：通过。
- 路由域单元测试：3 个通过。
- 410 当前部署的是 GitHub Actions 提交 `7481f9a`；本工作树的刷新并发保护与
  iptables/nft NAT fallback 尚未进入设备。提交后必须重新拉取 ARM64 Release，再进行
  实机 VoLTE/VoWiFi 来电复测。
