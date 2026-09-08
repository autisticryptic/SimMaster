# SimAdmin DNS 解析层 Hickory 重构任务

创建日期：2026-09-02
状态：DNS 排序修正 0b97b4f 和后续 beta2/48e37fc 均通过各自最终 CI、部署、应用 HTTP DNS 及 VoWiFi 原通道自然续期实测；48e37fc 当前在设备运行。双注册及未启用的专用业务不标为通过
本轮范围：替换 libc 系统解析、HTTP 和代理端点隐式解析，启用 system-config；专用 DNS 传输维持原行为

> 2026-09-08 更新：详见 `docs/DNS_HICKORY.md` 和 `plan.md`。
> 用户本轮要求的普通系统解析已使用 hickory-resolver 0.25.2。
> 下列原始“统一所有专用 DNS 报文/传输”项目保留为后续设计清单；
> 原有运营商 DNS、P-CSCF、NAPTR 和 SOCKS5 UDP DNS 已是 Rust 实现，
> 本次不借替换 lookup_host 改变它们的出口/回退策略，也不谎称这些传输已经重写。
> 后续统一全部传输的未勾选项不是本次移除 libc 系统解析的完成声明；
> 当前候选的 DNS 实机失败也不能因已通过单元测试而忽略。

## 目标

将 SimAdmin 当前分散的系统解析、手写 DNS、指定 DNS、SOCKS5 DNS 和
NAPTR 查询统一到可测试的纯 Rust 解析层，避免静态 musl 环境下
`lookup_host` 与 libc/NSS 行为不一致，同时保持 UE network namespace、
运营商指定 DNS 和代理出口的严格隔离。

## 当前临时实现

- [x] 未指定 DNS 时显式读取 `/etc/hosts`。
- [x] hosts 未命中时由 Hickory 读取系统配置并查询，不再调用系统 `lookup_host`。
- [x] 系统解析失败时保留现有 UDP DNS fallback。
- [x] 指定 Profile DNS 时仍严格使用指定 DNS，不读取 hosts 覆盖运营商配置。
- [ ] Hickory 重构完成后删除不再需要的临时重复实现。

## 目标解析顺序

### 明确指定 DNS

1. Profile 或线路指定的 DNS。
2. 按线路要求通过直连或 SOCKS5/代理出口查询。
3. 不自动使用宿主 DNS 覆盖明确配置。

### 未指定 DNS

1. `/etc/hosts`。
2. Hickory 读取的系统 resolver 配置。
3. 项目允许的公共 DNS fallback。
4. 返回结构化、可展示但简短的失败原因。

## 开发任务

### 1. 依赖与构建

- [x] 固定 `hickory-resolver 0.25.2`，与 reqwest 0.12 的 Hickory 版本一致。
- [x] 启用 `system-config`、`tokio`，hosts 使用 Hickory parser；HTTP 统一解析入口并启用 hickory-dns。
- [x] 更新 `backend/Cargo.toml` 与 `backend/Cargo.lock`；不在本机编译后端。
- [ ] 确认 ARM64、AMD64 的 musl 静态构建均不引入动态运行库依赖。
- [ ] 检查最终二进制体积和启动内存变化。

### 2. 统一抽象

- [ ] 建立线路级 `DnsResolver` 实现，禁止业务模块自行选择解析库。
- [ ] 将 A、AAAA、NAPTR 查询收敛到同一个解析入口。
- [ ] 将 hosts、系统配置、指定 DNS 和 fallback 表达为明确的策略对象。
- [ ] 解析结果保留来源：hosts、system、profile_dns、proxy_dns 或 fallback。
- [ ] 统一超时、重试、地址去重、IPv4/IPv6 顺序和错误分类。

### 3. UE namespace 与代理

- [ ] 确认 resolver 的 UDP/TCP socket 必须在对应 UE worker/network namespace 创建。
- [ ] 禁止在宿主 namespace 解析后错误地从另一线路发送运营商 DNS 请求。
- [ ] 保留 SOCKS5 UDP ASSOCIATE DNS 查询能力。
- [ ] 验证两个线路使用不同 DNS、不同代理时不会共享错误的解析状态。
- [ ] ePDG 地址缓存必须按线路、Profile、DNS 策略和地址族隔离。

### 4. 缓存与生命周期

- [ ] 定义正向缓存 TTL、负缓存 TTL 和最大条目数。
- [ ] hosts 结果不得被长时间缓存，确保文件修改后可在合理时间内生效。
- [ ] SIM/eSIM、ICCID、PLMN、Profile、DNS 或代理切换时清理对应线路缓存。
- [ ] IMS 会话结束、线路移除和进程退出时释放线路级 resolver 状态。
- [ ] 禁止无上限的全局缓存增长。

### 5. 迁移范围

- [ ] 迁移 VoWiFi ePDG A/AAAA 查询。
- [ ] 迁移 visited-country ePDG NAPTR 查询。
- [ ] 迁移指定运营商 DNS 查询。
- [ ] 迁移 SOCKS5 DNS 查询，或通过统一 transport adapter 接入。
- [ ] 评估 Trunk、E911/TS.43 等其他解析调用是否应复用同一组件。
- [ ] 所有调用方迁移完成后删除手写的重复 DNS 报文解析代码。

### 6. 测试

- [ ] hosts：IPv4、IPv6、别名、注释、大小写和末尾点。
- [ ] resolv.conf：多 nameserver、search/domain、无效配置和文件不存在。
- [ ] A、AAAA、NAPTR 的正常、空响应、NXDOMAIN、SERVFAIL 和超时。
- [ ] 指定 DNS 不被宿主 hosts 或宿主 DNS 覆盖。
- [ ] 未指定 DNS 时 hosts 优先级正确。
- [ ] DNS fallback 只在允许的错误类型下触发。
- [ ] UE namespace、双线路隔离、SOCKS5 和 IPv4/IPv6 回退测试。
- [x] 2bb6099 / 34169025117 与排序修正 0b97b4f / 34173433980 分别通过 GitHub Actions test、ARM64、AMD64、Release，没有沿用旧构建。
- [x] 六服务器场景中每次新建 resolver 保持系统顺序；0b97b4f 回归通过。08:54 应用向系统首台 DNS 查询 A/AAAA 并收到响应，HTTPS 元数据读取 0.496 秒成功，未修改 DNS 配置。

### 7. 410 实机验收

- [ ] 无自定义 Profile，仅派生 Vodafone Germany 配置能够解析 hosts 中的 ePDG。
- [x] 2bb6099 启动证据：IKE/500、CHILD_SA、ESP/TUN 和 IMS REGISTER 成功，07:35:32 获得 3553 秒租期；没有将未直接观察的 NAT-T/4500 单独标为已实测。
- [ ] 指定德国运营商 DNS 时仍能得到运营商预期结果。
- [ ] hosts、指定 DNS、系统 DNS 三种路径的日志能够区分来源。
- [x] 0b97b4f 在 09:31:28 自然续期复用原通道；CSeq=3、Security-Verify、真实剩余 531 秒，0.292738 秒后 200、续得 3100 秒；服务/TUN 未变，没有重建 ePDG/安全关联来代替 refresh。
- [ ] VoLTE 注册和续期不受 DNS 重构影响。

## 完成标准

- [x] Hickory 成为普通系统 DNS 的唯一实现入口；专用传输保留，不冒充已统一。
- [x] 项目不再直接依赖 `tokio::net::lookup_host` 处理 ePDG。
- [ ] 原有指定 DNS、SOCKS5、NAPTR、UE namespace 能力无回归。
- [x] 双架构 CI、410 VoWiFi 初始注册及至少一次原通道自然续期通过；不冒充当前网络双路注册/续期通过。
- [x] 更新架构文档并仅勾选有对应证据的本轮验收项；后续专用传输统一项目、未启用/未实测业务和未通过项继续保留。

## 非目标

- 本任务不改变 IMS Profile 的派生规则。
- 本任务不改变 ePDG/IKE/ESP、SIP REGISTER 或 AKA 协议实现。
- 本任务不把 DNS 缓存写入业务数据库。
- 本任务不把 glibc 动态链接作为解决方案；项目继续保留 musl 静态发布。
